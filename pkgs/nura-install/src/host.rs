//! The host side: files and hashes, downloads, the private cache, prerequisites.
use crate::cmd::{self, capture, stdout_of, Spec};
use crate::config;
use crate::fail::Result;
use crate::log;
use crate::render::human;
use crate::ui::{self, Gauge};
use std::ffi::OsString;
use std::fs::{self, File, TryLockError};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;
use std::process::Command;

const ALLOWED_HOSTS: [&str; 2] = ["images.nura.eco", "images.ecloud.global"];

pub fn mkdir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("creating {}: {e}", dir.display()).into())
}

/// Makes what the owner cannot get again durable before the run goes on: fsync of every file in `dir`
/// (and in the directories below it), then of each directory up to and including `root`, so the new
/// entries survive a power cut too. Without it, a crash after the fastboot writes could leave a
/// backup the record names but the disk never got.
pub fn sync_tree(dir: &Path, root: &Path) -> Result<()> {
    sync_below(dir)?;
    let mut at = Some(dir);
    while let Some(d) = at {
        sync_path(d)?;
        at = (d != root).then(|| d.parent()).flatten();
    }
    checked("synced to disk", &dir.display().to_string());
    Ok(())
}

fn sync_below(dir: &Path) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("reading {}: {e}", dir.display()))?
            .path();
        if path.is_dir() {
            sync_below(&path)?;
        }
        sync_path(&path)?;
    }
    Ok(())
}

/// fsync of one file or directory.
pub fn sync_path(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("syncing {}: {e}", path.display()).into())
}

pub fn remove(path: &Path) -> Result<()> {
    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() || path.is_symlink() {
        fs::remove_file(path)
    } else {
        return Ok(());
    };
    result.map_err(|e| format!("removing {}: {e}", path.display()).into())
}

pub fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn sha256(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let total = fs::metadata(path).ok().map(|m| m.len());
    let _shown = ui::busy(&format!("hashing {name}"), Gauge::read(path, total));
    let out = stdout_of(Command::new("sha256sum").arg("--").arg(path))?;
    out.split_whitespace()
        .next()
        .filter(|s| is_sha256(s))
        .map(String::from)
        .ok_or_else(|| format!("sha256sum printed no hash for {}", path.display()).into())
}

#[derive(Clone, Copy)]
pub enum Schemes {
    HttpsOnly,
    HttpsAndFile,
}

fn schemes() -> Schemes {
    if config::allow_file_urls() {
        Schemes::HttpsAndFile
    } else {
        Schemes::HttpsOnly
    }
}

pub fn check_url(url: &str, schemes: Schemes) -> Result<()> {
    if matches!(schemes, Schemes::HttpsAndFile) && url.starts_with("file:///") {
        return Ok(());
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("refusing {url}: only https downloads are allowed"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if ALLOWED_HOSTS.contains(&authority) {
        Ok(())
    } else {
        Err(format!(
            "refusing {url}: downloads may only come from {}",
            ALLOWED_HOSTS.join(" and ")
        )
        .into())
    }
}

/// Fail a stalled connection or transfer, without a total limit: the images are gigabytes.
const STALL_LIMITS: [&str; 6] = [
    "--connect-timeout",
    "30",
    "--speed-limit",
    "1024",
    "--speed-time",
    "60",
];

/// `-q` first so ~/.curlrc is ignored; no redirects, since one could leave the allowed hosts.
fn curl(url: &str) -> Result<Command> {
    let schemes = schemes();
    check_url(url, schemes)?;
    let mut cmd = Command::new(config::curl_bin());
    cmd.arg("-q")
        .args(STALL_LIMITS)
        .args([
            "-fL",
            "--max-redirs",
            "0",
            "--proto-redir",
            "=https",
            "--proto",
        ])
        .arg(match schemes {
            Schemes::HttpsOnly => "=https",
            Schemes::HttpsAndFile => "=https,file",
        });
    Ok(cmd)
}

fn head(url: &str) -> Result<cmd::Done> {
    let schemes = schemes();
    check_url(url, schemes)?;
    let mut cmd = Command::new(config::curl_bin());
    cmd.args([
        "-q",
        "-sI",
        "--connect-timeout",
        "30",
        "--max-time",
        "60",
        "--max-redirs",
        "0",
        "--proto-redir",
        "=https",
        "--proto",
    ])
    .arg(match schemes {
        Schemes::HttpsOnly => "=https",
        Schemes::HttpsAndFile => "=https,file",
    })
    .arg("--")
    .arg(url);
    capture(&mut cmd)
}

/// The size of a download when the server (or the local file) says it; `None` is shown as unknown.
pub fn download_size(url: &str) -> Option<u64> {
    if let Some(path) = url.strip_prefix("file://") {
        return fs::metadata(path).ok().map(|m| m.len());
    }
    let out = head(url).ok().filter(cmd::Done::ok)?;
    out.stdout.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    })
}

pub fn curl_file(url: &str, dest: &Path) -> Result<()> {
    let mut cmd = curl(url)?;
    cmd.arg("-sS").arg("-o").arg(dest).arg("--").arg(url);
    cmd::run(&mut cmd, Spec::default())?
        .success()
        .map(drop)
        .map_err(|e| format!("downloading {url}: {e}").into())
}

pub fn file_name_of(url: &str) -> Result<String> {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or("");
    let valid = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b));
    if valid {
        Ok(name.to_string())
    } else {
        Err(format!("{url} has no usable file name").into())
    }
}

/// Refuses root, sets the private umask and returns the effective user id.
pub fn harden_process() -> Result<u32> {
    // SAFETY: geteuid takes no arguments and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return Err("refusing to run as root: run it as your own user".into());
    }
    // SAFETY: umask takes a plain mode and always succeeds; the previous mask is not needed.
    let _ = unsafe { libc::umask(0o077) };
    Ok(euid)
}

pub fn ensure_cache(dir: &Path, euid: u32) -> Result<()> {
    if !dir.exists() {
        return mkdir(dir);
    }
    let meta = fs::metadata(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != euid || meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be a directory owned by you and private; run `chmod 700 {}`",
            dir.display(),
            dir.display()
        )
        .into());
    }
    Ok(())
}

pub fn lock(cache: &Path) -> Result<File> {
    let path = cache.join("lock");
    let file = File::create(&path).map_err(|e| format!("creating {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => {
            Err(format!("another nura-install is using {}", cache.display()).into())
        }
        Err(TryLockError::Error(e)) => Err(format!("locking {}: {e}", path.display()).into()),
    }
}

pub fn timestamp() -> Result<String> {
    Ok(
        stdout_of(Command::new("date").args(["-u", "+%Y%m%dT%H%M%SZ"]))?
            .trim()
            .to_string(),
    )
}

struct Tool {
    program: OsString,
    arg: &'static str,
}

/// What the run needs on this computer: every program starts.
pub fn check_tools() -> Result<((), String)> {
    let tool = |program: OsString, arg| Tool { program, arg };
    let plain = |name: &str, arg| tool(name.into(), arg);
    let tools = [
        tool(config::adb_bin(), "version"),
        tool(config::fastboot_bin(), "--version"),
        tool(config::curl_bin(), "--version"),
        plain("jq", "--version"),
        plain("sha256sum", "--version"),
        plain("xz", "--version"),
        plain("tar", "--version"),
        plain("date", "--version"),
        plain("df", "--version"),
        plain("cut", "--version"),
    ];
    let count = tools.len();
    for tool in tools {
        let name = Path::new(&tool.program).display().to_string();
        let out = capture(Command::new(&tool.program).arg(tool.arg))
            .map_err(|e| format!("prerequisite {name} does not run: {e}"))?;
        out.success()
            .map_err(|e| format!("prerequisite {name} does not run: {e}"))?;
    }
    Ok(((), format!("all {count} are there")))
}

pub fn tools_naming() -> String {
    "checking that the programs this run needs start".to_string()
}

/// The cache's disk must have room for the downloads and the backup.
pub fn check_disk(cache: &Path) -> Result<((), String)> {
    let free = free_bytes(cache)?;
    let needed = config::needed_gib()?;
    if free / (1 << 30) < needed {
        return Err(format!(
            "{} has {} GiB free; the run needs about {needed} GiB. Next: free space there, or move the cache, then run again",
            cache.display(),
            free / (1 << 30)
        )
        .into());
    }
    Ok((
        (),
        format!(
            "{} free in {}, the run needs about {needed} GiB",
            human(free),
            cache.display()
        ),
    ))
}

fn free_bytes(dir: &Path) -> Result<u64> {
    let out = stdout_of(Command::new("df").args(["-B1", "--output=avail"]).arg(dir))?;
    out.lines()
        .filter_map(|l| l.trim().parse().ok())
        .next_back()
        .ok_or_else(|| format!("df printed no free space for {}", dir.display()).into())
}

/// Records a passed check and its value in the run transcript.
pub fn checked(what: &str, value: &str) {
    log::line(&format!("check {what}: {value}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_are_https_from_allowed_hosts_only() {
        let ok = |u: &str| check_url(u, Schemes::HttpsOnly).is_ok();
        assert!(ok("https://images.nura.eco/bpo/index.json"));
        assert!(ok("https://images.ecloud.global/x?y=1"));
        assert!(!ok("http://images.nura.eco/x"));
        assert!(!ok("https://evil.example/x"));
        assert!(!ok("https://images.nura.eco.evil.example/x"));
        assert!(!ok("https://images.nura.eco@evil.example/x"));
        assert!(!ok("https://images.nura.eco:444/x"));
        assert!(!ok("file:///etc/passwd"));
        assert!(check_url("file:///tmp/x", Schemes::HttpsAndFile).is_ok());
        assert!(check_url("file:///tmp/x", Schemes::HttpsOnly).is_err());
    }

    #[test]
    fn file_names_come_from_the_url_path() {
        assert_eq!(
            file_name_of("https://h/a/b-1.img.xz?x=1").unwrap(),
            "b-1.img.xz"
        );
        assert!(file_name_of("https://h/a/").is_err());
        assert!(file_name_of("https://h/a/..").is_err());
        assert!(file_name_of("https://h/a/b c").is_err());
    }

    #[test]
    fn curl_ignores_curlrc_and_never_follows_redirects() {
        let cmd = curl("https://images.nura.eco/x").unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "-q");
        let joined = args.join(" ");
        assert!(joined.contains("--max-redirs 0"));
        assert!(joined.contains("--proto-redir =https"));
        assert!(joined.contains("--proto =https"));
        assert!(joined.contains("--connect-timeout 30"));
        assert!(joined.contains("--speed-limit 1024 --speed-time 60"));
        assert!(!joined.contains("--max-time"));
    }

    #[test]
    fn sync_tree_reaches_files_and_every_directory_up_to_the_root_and_fails_loud() {
        let root = std::env::temp_dir().join(format!("nura-sync-{}", std::process::id()));
        let dir = root.join("a").join("b");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("f"), "x").unwrap();
        assert!(sync_tree(&dir, &root).is_ok());
        assert!(sync_tree(&root.join("missing"), &root).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sha256_shape() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"A".repeat(64)));
        assert!(!is_sha256("abc"));
    }
}
