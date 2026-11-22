//! Noting Ctrl-C, hang-up and terminate.
use crate::fail::Result;
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const GUARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGHUP, libc::SIGTERM];

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static SIGNAL: AtomicI32 = AtomicI32::new(libc::SIGINT);

extern "C" fn note_interrupt(sig: libc::c_int) {
    SIGNAL.store(sig, Ordering::Relaxed);
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Ctrl-C, hang-up and terminate only set a flag; the heartbeat thread then reports and exits.
pub fn install_interrupt() -> Result<()> {
    // SAFETY: sigaction gets a zeroed struct whose handler only stores into atomics.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = note_interrupt as extern "C" fn(libc::c_int) as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        for sig in GUARDED {
            if libc::sigaction(sig, &action, ptr::null_mut()) != 0 {
                return Err(
                    format!("catching signal {sig}: {}", io::Error::last_os_error()).into(),
                );
            }
        }
    }
    Ok(())
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// The shell's convention: 128 plus the signal that came.
pub fn exit_code() -> u8 {
    u8::try_from(128 + SIGNAL.load(Ordering::Relaxed)).unwrap_or(130)
}
