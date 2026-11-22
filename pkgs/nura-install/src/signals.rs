//! Holding Ctrl-C, hang-up and terminate off while a group of writes runs, and noting them otherwise.
use crate::fail::Result;
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};

const GUARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGHUP, libc::SIGTERM];

fn guarded_set() -> Result<libc::sigset_t> {
    // SAFETY: sigset_t is plain data, initialized by sigemptyset before any other use.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        if libc::sigemptyset(&mut set) != 0 {
            return Err(format!("building the signal set: {}", io::Error::last_os_error()).into());
        }
        for sig in GUARDED {
            if libc::sigaddset(&mut set, sig) != 0 {
                return Err(
                    format!("building the signal set: {}", io::Error::last_os_error()).into(),
                );
            }
        }
        Ok(set)
    }
}

/// Holds the signals until dropped. A child started meanwhile inherits the block: `std::process::Command`
/// leaves the signal mask alone on this toolchain (the e2e check reads `SigBlk` in a fake fastboot).
pub struct SignalBlock {
    old: libc::sigset_t,
}

impl SignalBlock {
    pub fn new() -> Result<Self> {
        let set = guarded_set()?;
        // SAFETY: all-zero is a valid sigset_t, and sigprocmask fills `old` and reads `set` for this call.
        let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigprocmask(libc::SIG_BLOCK, &set, &mut old) } != 0 {
            return Err(format!("blocking signals: {}", io::Error::last_os_error()).into());
        }
        IN_GROUP.store(true, Ordering::Relaxed);
        Ok(Self { old })
    }

    /// Consumes the guarded signals that arrived meanwhile, so unblocking does not kill the process
    /// before it can report; true if there were any.
    pub fn take_pending(&self) -> Result<bool> {
        let set = guarded_set()?;
        let now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut any = false;
        loop {
            // SAFETY: `set` and `now` are valid for the call; the siginfo pointer may be null.
            let sig = unsafe { libc::sigtimedwait(&set, ptr::null_mut(), &now) };
            if sig >= 0 {
                any = true;
                continue;
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN) {
                return Ok(any);
            }
            return Err(format!("reading pending signals: {err}").into());
        }
    }
}

impl Drop for SignalBlock {
    fn drop(&mut self) {
        IN_GROUP.store(false, Ordering::Relaxed);
        // SAFETY: restores the mask saved in `new`; the result is ignored because SIG_SETMASK with a
        // valid saved set cannot fail and a destructor has no way to report it.
        let _ = unsafe { libc::sigprocmask(libc::SIG_SETMASK, &self.old, ptr::null_mut()) };
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static SIGNAL: AtomicI32 = AtomicI32::new(libc::SIGINT);
static IN_GROUP: AtomicBool = AtomicBool::new(false);
static COPY: AtomicU8 = AtomicU8::new(Copy::Off as u8);

extern "C" fn note_interrupt(sig: libc::c_int) {
    SIGNAL.store(sig, Ordering::Relaxed);
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Where the copy over ssh is, which decides what an interrupt does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Copy {
    Off = 0,
    /// Commands run on the phone: an interrupt ends the running command so sshd can be stopped.
    Session = 1,
    /// sshd is being stopped: an interrupt waits for that to finish.
    Stopping = 2,
}

pub fn set_copy(phase: Copy) {
    COPY.store(phase as u8, Ordering::Relaxed);
}

pub fn copy_phase() -> Copy {
    match COPY.load(Ordering::Relaxed) {
        1 => Copy::Session,
        2 => Copy::Stopping,
        _ => Copy::Off,
    }
}

/// Ctrl-C, hang-up and terminate outside a write group only set a flag; the heartbeat thread then
/// reports and exits.
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

/// Forgets an interrupt the run has handled itself (the end of the owner's wait for sshd).
pub fn clear_interrupt() {
    INTERRUPTED.store(false, Ordering::Relaxed);
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// The shell's convention: 128 plus the signal that came.
pub fn exit_code() -> u8 {
    u8::try_from(128 + SIGNAL.load(Ordering::Relaxed)).unwrap_or(130)
}

pub fn in_group() -> bool {
    IN_GROUP.load(Ordering::Relaxed)
}

/// The heartbeat thread never takes a guarded signal, so a held one waits for the main thread.
pub fn block_in_this_thread() {
    if let Ok(set) = guarded_set() {
        // SAFETY: `set` is valid for the call and the old mask is not wanted.
        let _ = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, ptr::null_mut()) };
    }
}

/// True when a guarded signal arrived while the group holds them.
pub fn pending() -> bool {
    // SAFETY: sigpending fills a zeroed set; sigismember only reads it.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut set) == 0 && GUARDED.iter().any(|s| libc::sigismember(&set, *s) == 1)
    }
}
