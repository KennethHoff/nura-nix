//! The one way to run an external program: logs the command line and its exit status and captures
//! its output.
use crate::fail::Result;
use crate::log;
use crate::ui;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

pub struct Done {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
    program: String,
}

impl Done {
    pub fn ok(&self) -> bool {
        self.status.success()
    }

    pub fn success(self) -> Result<Self> {
        if !self.status.success() {
            return Err(format!(
                "{} failed ({}): {}",
                self.program,
                self.status,
                self.stderr.trim()
            )
            .into());
        }
        Ok(self)
    }
}

fn describe(cmd: &Command) -> String {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collects everything the pipe delivers.
fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut all = Vec::new();
        pipe.read_to_end(&mut all)?;
        Ok(all)
    })
}

pub fn capture(cmd: &mut Command) -> Result<Done> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let line = describe(cmd);
    log::line(&format!("run: {line}"));
    let _shown = ui::ensure(&format!("running {line}"));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("running {program}: {e}"))?;
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    ui::set_child(child.id());
    let waited = child.wait();
    ui::set_child(0);
    let status = waited.map_err(|e| format!("waiting for {program}: {e}"))?;
    log::line(&format!("exit: {line} -> {status}"));
    let text = |handle: Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>| -> Result<String> {
        match handle {
            None => Ok(String::new()),
            Some(h) => {
                let bytes = h
                    .join()
                    .map_err(|_| format!("reading the output of {program} panicked"))?
                    .map_err(|e| format!("reading the output of {program}: {e}"))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
        }
    };
    Ok(Done {
        stdout: text(out)?,
        stderr: text(err)?,
        status,
        program: program.clone(),
    })
}

/// Stdout of a command that must succeed.
pub fn stdout_of(cmd: &mut Command) -> Result<String> {
    Ok(capture(cmd)?.success()?.stdout)
}
