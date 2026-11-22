//! The host side: files and hashes, downloads, the private cache, prerequisites.
use crate::cmd::{capture, stdout_of};
use crate::config;
use crate::fail::Result;
use crate::render::human;
use std::ffi::OsString;
use std::fs::{self, File, TryLockError};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;
use std::process::Command;

pub fn mkdir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("creating {}: {e}", dir.display()).into())
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
    let tools = [plain("date", "--version"), plain("df", "--version")];
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

/// The cache's disk must have room for the downloads.
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
