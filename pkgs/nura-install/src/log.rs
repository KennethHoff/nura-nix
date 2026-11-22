//! The run transcript: `<cache>/logs/<UTC timestamp>-<os>.log`, private, one line per event.
use crate::fail::Result;
use crate::host::mkdir;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

static LOG: Mutex<Option<File>> = Mutex::new(None);

pub fn open(cache: &Path, stamp: &str, os: &str) -> Result<()> {
    let dir = cache.join("logs");
    mkdir(&dir)?;
    let path = dir.join(format!("{stamp}-{os}.log"));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("creating the run log {}: {e}", path.display()))?;
    *LOG.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
    Ok(())
}

pub fn line(msg: &str) {
    let mut guard = LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(file) = guard.as_mut() {
        if let Err(e) = writeln!(file, "{msg}") {
            eprintln!("nura-install: writing the run log: {e}");
        }
    }
}
