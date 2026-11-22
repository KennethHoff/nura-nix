//! Downloads and unpacking, always checked against a hash: Nura's index. A `Verified` file can only be
//! made here, after its hash matched, and is hashed again right before it is written to the phone.
use crate::cmd::to_file;
use crate::fail::Result;
use crate::host::{curl_file, download_size, file_name_of, mkdir, remove, sha256};
use crate::nura::{Build, Pin};
use crate::render::human;
use crate::ui::{self, Gauge, Job};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Verified {
    path: PathBuf,
    sha256: String,
    against: &'static str,
}

impl Verified {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn rehash(&self) -> Result<()> {
        let name = name_of(&self.path);
        let size = size_text(&self.path);
        ui::run(
            Job::new(&format!("hash of {name}")).naming(format!(
                "re-hashing {name} ({size}) right before it is written"
            )),
            || {
                let got = sha256(&self.path)?;
                if got == self.sha256 {
                    Ok(((), format!("matches {}", self.against)))
                } else {
                    Err(mismatch(&self.path, &self.sha256, &got, self.against).into())
                }
            },
        )
    }
}

pub struct NuraImages {
    pub boot: Verified,
    pub rootfs: Verified,
}

pub const INDEX: &str = "Nura's index";
pub const UNPACKED: &str = "the hash taken when it was unpacked";

fn name_of(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn size_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn size_text(path: &Path) -> String {
    size_of(path).map_or_else(|| "size unknown".to_string(), human)
}

fn mismatch(what: &Path, want: &str, got: &str, against: &str) -> String {
    format!(
        "{} does not match {against} (want {want}, got {got}); nothing was written to the phone",
        what.display()
    )
}

fn part_of(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Fills `dest.part` with `make`, keeps it as `dest` only if it hashes to `want`.
fn publish(
    dest: &Path,
    want: &str,
    against: &'static str,
    make: impl FnOnce(&Path) -> Result<()>,
) -> Result<Verified> {
    let part = part_of(dest);
    remove(&part)?;
    make(&part)?;
    let got = sha256(&part)?;
    if got != want {
        remove(&part)?;
        return Err(mismatch(dest, want, &got, against).into());
    }
    std::fs::rename(&part, dest).map_err(|e| format!("moving {}: {e}", part.display()))?;
    Ok(Verified {
        path: dest.to_path_buf(),
        sha256: want.to_string(),
        against,
    })
}

/// A file kept from an earlier run, hashed again as a step of its own because that takes a while.
fn cached(dest: &Path, want: &str, against: &'static str) -> Result<Option<Verified>> {
    if !dest.exists() {
        return Ok(None);
    }
    let name = name_of(dest);
    let size = size_text(dest);
    let hit = ui::run(
        Job::new(&format!("cached {name}"))
            .naming(format!("re-hashing the cached {name} ({size})")),
        || {
            if sha256(dest)? == want {
                Ok((true, "hash matches".to_string()))
            } else {
                Ok((false, "hash differs, fetching it again".to_string()))
            }
        },
    )?;
    Ok(hit.then(|| Verified {
        path: dest.to_path_buf(),
        sha256: want.to_string(),
        against,
    }))
}

/// Re-hashes a cached file on every run.
pub fn fetch(dir: &Path, pin: &Pin, against: &'static str) -> Result<Verified> {
    mkdir(dir)?;
    let name = file_name_of(&pin.url)?;
    let dest = dir.join(&name);
    if let Some(file) = cached(&dest, &pin.sha256, against)? {
        return Ok(file);
    }
    let size = download_size(&pin.url);
    let shown = size.map_or_else(|| "size unknown".to_string(), human);
    ui::run(
        Job::new(&format!("download {name}")).naming(format!("downloading {name} ({shown})")),
        || {
            let file = publish(&dest, &pin.sha256, against, |part| {
                ui::set_gauge(Gauge::written(part, size));
                curl_file(&pin.url, part)
            })?;
            Ok((
                file,
                format!("{}, hash matches {against}", size_text(&dest)),
            ))
        },
    )
}

/// Unpacks a verified download and hashes the result itself: the index vouches for the compressed
/// file only. A raw file left by an earlier run is replaced, never trusted.
fn unpack_xz(xz: &Verified) -> Result<Verified> {
    let raw = xz.path.with_extension("");
    let name = name_of(&xz.path);
    let size = size_text(&xz.path);
    ui::run(
        Job::new(&format!("unpack {name}")).naming(format!("unpacking {name} ({size} compressed)")),
        || {
            let part = part_of(&raw);
            remove(&part)?;
            ui::set_gauge(Gauge::read(&xz.path, size_of(&xz.path)));
            to_file(
                Command::new("xz").arg("-dc").arg("--").arg(&xz.path),
                &part,
                None,
            )?
            .success()
            .map(drop)?;
            let got = sha256(&part)?;
            std::fs::rename(&part, &raw).map_err(|e| format!("moving {}: {e}", part.display()))?;
            let file = Verified {
                path: raw.clone(),
                sha256: got,
                against: UNPACKED,
            };
            Ok((file, format!("{}, hashed", size_text(&raw))))
        },
    )
}

pub fn nura(cache: &Path, build: &Build) -> Result<NuraImages> {
    let dir = cache.join("downloads");
    let boot = fetch(&dir, &build.boot, INDEX)?;
    let rootfs = fetch(&dir, &build.rootfs, INDEX)?;
    Ok(NuraImages {
        boot: unpack_xz(&boot)?,
        rootfs: unpack_xz(&rootfs)?,
    })
}
