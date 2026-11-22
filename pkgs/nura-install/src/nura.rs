//! Nura's own image index: which interfaces it builds for the phone and the newest build of each.
//! The index is fetched fresh on every run and read with jq; what it says about a file (its URL
//! and SHA256) is checked when the file is downloaded (see `images.rs`).
use crate::config;
use crate::fail::Result;
use crate::host::{curl_file, is_sha256, mkdir, remove};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cmd::stdout_of;

/// What a phone calls itself in fastboot (`product`), and Nura's name for it.
const DEVICES: [(&str, &str); 1] = [("FP6", "fairphone-fp6")];

/// An interface Nura builds: the word the owner types, its name in the index and a line to choose by.
struct Known {
    label: &'static str,
    word: &'static str,
    name: &'static str,
    about: &'static str,
}

const KNOWN: [Known; 4] = [
    Known {
        label: "Phosh",
        word: "phosh",
        name: "phosh",
        about: "Phosh (GNOME's phone shell): simple, the most used",
    },
    Known {
        label: "Plasma Mobile",
        word: "plasma",
        name: "plasma-mobile",
        about: "KDE Plasma Mobile: smartphone-like, the heaviest",
    },
    Known {
        label: "GNOME Mobile",
        word: "gnome",
        name: "gnome-mobile",
        about: "GNOME Mobile: the GNOME desktop adapted to phones",
    },
    Known {
        label: "Sxmo (Sway)",
        word: "sxmo",
        name: "sxmo-de-sway",
        about: "Sxmo on Sway: minimal, driven by buttons and gestures",
    },
];

const FLATTEN: &str = r#".releases[] | .name as $r | .devices[] | select(.name == $dev) | .interfaces[] | .name as $i | .images[] | [$r, $i, .name, .timestamp, .url, .sha256] | map(if type == "string" then . else "" end) | @tsv"#;

pub struct Pin {
    pub url: String,
    pub sha256: String,
}

/// One build of one interface: the two images that were built together.
pub struct Build {
    pub release: String,
    pub interface: String,
    pub timestamp: String,
    pub boot: Pin,
    pub rootfs: Pin,
}

impl Build {
    /// What the record and the output call the build.
    pub fn label(&self) -> String {
        format!("{} {} {}", self.release, self.interface, self.timestamp)
    }
}

/// An interface the owner can pick, with the newest build of it.
pub struct Offer {
    pub label: String,
    pub word: String,
    pub about: String,
    pub build: Build,
}

pub fn device_for(product: &str) -> Result<&'static str> {
    DEVICES
        .iter()
        .find(|(p, _)| *p == product)
        .map(|(_, device)| *device)
        .ok_or_else(|| {
            format!(
                "this tool has no Nura device for a phone whose fastboot product is {product:?}"
            )
            .into()
        })
}

/// Downloads the index afresh into the cache and returns its path.
pub fn fetch_index(cache: &Path) -> Result<PathBuf> {
    let dir = cache.join("downloads");
    mkdir(&dir)?;
    let path = dir.join("index.json");
    remove(&path)?;
    curl_file(&config::nura_index(), &path)?;
    Ok(path)
}

pub fn offers(index: &Path, device: &str) -> Result<Vec<Offer>> {
    let tsv = stdout_of(
        Command::new("jq")
            .args(["-r", "--arg", "dev", device, FLATTEN])
            .arg(index),
    )
    .map_err(|e| format!("reading Nura's index {}: {e}", index.display()))?;
    parse(&tsv, device)
}

struct Row<'a> {
    release: &'a str,
    interface: &'a str,
    name: &'a str,
    timestamp: &'a str,
    url: &'a str,
    sha256: &'a str,
}

fn rows(tsv: &str) -> Result<Vec<Row<'_>>> {
    tsv.lines()
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            match f[..] {
                [release, interface, name, timestamp, url, sha256] => Ok(Row {
                    release,
                    interface,
                    name,
                    timestamp,
                    url,
                    sha256,
                }),
                _ => Err(format!("a malformed image entry in Nura's index: {line:?}").into()),
            }
        })
        .collect()
}

fn pin(row: &Row) -> Result<Pin> {
    if row.url.is_empty() || !is_sha256(row.sha256) {
        return Err(format!(
            "Nura's index has no URL or no valid sha256 for {}",
            row.name
        )
        .into());
    }
    Ok(Pin {
        url: row.url.to_string(),
        sha256: row.sha256.to_string(),
    })
}

/// The newest complete build (a boot image and a root filesystem with one timestamp) of each
/// interface across the releases, in the order the owner is shown them.
fn parse(tsv: &str, device: &str) -> Result<Vec<Offer>> {
    let rows = rows(tsv)?;
    if rows.is_empty() {
        return Err(format!("Nura's index lists no images for {device}").into());
    }
    let mut interfaces: Vec<&str> = Vec::new();
    for row in &rows {
        if !interfaces.contains(&row.interface) {
            interfaces.push(row.interface);
        }
    }
    interfaces.sort_by_key(|name| {
        KNOWN
            .iter()
            .position(|k| k.name == *name)
            .unwrap_or(KNOWN.len())
    });
    let mut offers = Vec::new();
    for interface in interfaces {
        let mine: Vec<&Row> = rows.iter().filter(|r| r.interface == interface).collect();
        let mut newest: Option<(&Row, &Row)> = None;
        for boot in mine.iter().filter(|r| r.name.ends_with("-boot.img.xz")) {
            let rootfs: Vec<&&Row> = mine
                .iter()
                .filter(|r| {
                    r.name.ends_with(".img.xz")
                        && !r.name.ends_with("-boot.img.xz")
                        && r.release == boot.release
                        && r.timestamp == boot.timestamp
                })
                .collect();
            let [rootfs] = rootfs[..] else {
                if rootfs.len() > 1 {
                    return Err(format!(
                        "Nura's index lists {} root filesystems for {} in {} at {}",
                        rootfs.len(),
                        interface,
                        boot.release,
                        boot.timestamp
                    )
                    .into());
                }
                continue;
            };
            if boot.timestamp.is_empty() {
                continue;
            }
            if newest.is_none_or(|(b, _)| boot.timestamp > b.timestamp) {
                newest = Some((boot, rootfs));
            }
        }
        let Some((boot, rootfs)) = newest else {
            continue;
        };
        let known = KNOWN.iter().find(|k| k.name == interface);
        offers.push(Offer {
            label: known.map_or(interface, |k| k.label).to_string(),
            word: known.map_or(interface, |k| k.word).to_string(),
            about: known.map_or_else(String::new, |k| k.about.to_string()),
            build: Build {
                release: boot.release.to_string(),
                interface: interface.to_string(),
                timestamp: boot.timestamp.to_string(),
                boot: pin(boot)?,
                rootfs: pin(rootfs)?,
            },
        });
    }
    if offers.is_empty() {
        return Err(format!(
            "Nura's index lists no complete build (boot image and root filesystem) for {device}"
        )
        .into());
    }
    Ok(offers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(release: &str, interface: &str, name: &str, ts: &str) -> String {
        let sha = "a".repeat(64);
        format!("{release}\t{interface}\t{name}\t{ts}\thttps://images.nura.eco/{name}\t{sha}\n")
    }

    fn build(release: &str, interface: &str, ts: &str) -> String {
        row(release, interface, &format!("{ts}-boot.img.xz"), ts)
            + &row(release, interface, &format!("{ts}.img.xz"), ts)
    }

    #[test]
    fn the_newest_build_wins_across_releases() {
        let tsv = build("edge", "phosh", "2026-11-06T17:02:00")
            + &build("edge", "phosh", "2026-11-13T23:32:00")
            + &build("v27.01", "phosh", "2026-12-01T10:00:00")
            + &build("v26.06", "phosh", "2026-11-20T10:00:00");
        let offers = parse(&tsv, "d").unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].build.release, "v27.01");
        assert_eq!(offers[0].build.label(), "v27.01 phosh 2026-12-01T10:00:00");
    }

    #[test]
    fn a_boot_image_without_its_rootfs_is_not_a_build() {
        let tsv = build("edge", "phosh", "2026-11-06T17:02:00")
            + &row(
                "edge",
                "phosh",
                "2026-11-13T23:32:00-boot.img.xz",
                "2026-11-13T23:32:00",
            );
        assert_eq!(
            parse(&tsv, "d").unwrap()[0].build.timestamp,
            "2026-11-06T17:02:00"
        );
        assert!(parse(&row("edge", "phosh", "x-boot.img.xz", "t"), "d").is_err());
    }

    #[test]
    fn known_interfaces_come_first_in_their_order_and_unknown_ones_keep_their_name() {
        let tsv = build("edge", "lomiri", "2026-11-06T17:02:00")
            + &build("edge", "sxmo-de-sway", "2026-11-06T17:02:00")
            + &build("edge", "plasma-mobile", "2026-11-06T17:02:00");
        let offers = parse(&tsv, "d").unwrap();
        let words: Vec<&str> = offers.iter().map(|o| o.word.as_str()).collect();
        assert_eq!(words, ["plasma", "sxmo", "lomiri"]);
        assert_eq!(offers[0].label, "Plasma Mobile");
        assert_eq!(offers[2].about, "");
    }

    #[test]
    fn the_chosen_build_needs_a_url_and_a_sha256() {
        let bad = build("edge", "phosh", "t").replace(&"a".repeat(64), "short");
        assert!(parse(&bad, "d").is_err());
    }

    #[test]
    fn two_root_filesystems_for_one_build_are_refused() {
        let tsv = build("edge", "phosh", "t") + &row("edge", "phosh", "other.img.xz", "t");
        assert!(parse(&tsv, "d").is_err());
    }

    #[test]
    fn an_empty_listing_and_an_unknown_phone_are_errors() {
        assert!(parse("", "d").is_err());
        assert!(device_for("FP6").is_ok());
        assert!(device_for("PIXEL").is_err());
    }
}
