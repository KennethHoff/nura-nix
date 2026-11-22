//! The one way to run an external program: logs the command line and its exit status, can capture or
//! redirect output, feed stdin and kill the child after a timeout.
use crate::fail::Result;
use crate::log;
use crate::ui;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

pub enum Sink<'a> {
    Capture,
    File(&'a Path),
}

pub struct Spec<'a> {
    pub stdin: Option<&'a str>,
    pub stdout: Sink<'a>,
    pub stderr: Sink<'a>,
    pub timeout: Option<Duration>,
}

impl Default for Spec<'_> {
    fn default() -> Self {
        Spec {
            stdin: None,
            stdout: Sink::Capture,
            stderr: Sink::Capture,
            timeout: None,
        }
    }
}

pub struct Done {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
    program: String,
    timed_out: Option<Duration>,
}

impl Done {
    pub fn ok(&self) -> bool {
        self.timed_out.is_none() && self.status.success()
    }

    pub fn success(self) -> Result<Self> {
        if let Some(limit) = self.timed_out {
            return Err(format!(
                "{} did not finish within {} seconds and was killed; whether it took effect is unknown",
                self.program,
                limit.as_secs()
            )
            .into());
        }
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

fn stdio(sink: &Sink) -> Result<Stdio> {
    Ok(match sink {
        Sink::Capture => Stdio::piped(),
        Sink::File(path) => Stdio::from(
            File::create(path).map_err(|e| format!("creating {}: {e}", path.display()))?,
        ),
    })
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

fn wait(
    child: &mut std::process::Child,
    program: &str,
    limit: Option<Duration>,
) -> Result<(ExitStatus, Option<Duration>)> {
    let err = |e: std::io::Error| format!("waiting for {program}: {e}");
    let Some(limit) = limit else {
        return Ok((child.wait().map_err(err)?, None));
    };
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(err)? {
            return Ok((status, None));
        }
        if started.elapsed() >= limit {
            // A failing kill means the child just exited; the wait below reports how.
            let _ = child.kill();
            return Ok((child.wait().map_err(err)?, Some(limit)));
        }
        sleep(Duration::from_millis(100));
    }
}

pub fn run(cmd: &mut Command, spec: Spec) -> Result<Done> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let line = describe(cmd);
    log::line(&format!("run: {line}"));
    let _shown = ui::ensure(&format!("running {line}"));
    cmd.stdin(if spec.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(stdio(&spec.stdout)?)
    .stderr(stdio(&spec.stderr)?);
    let mut child = cmd.spawn().map_err(|e| format!("running {program}: {e}"))?;
    let writer = spec.stdin.zip(child.stdin.take()).map(|(input, mut pipe)| {
        let data = input.as_bytes().to_vec();
        thread::spawn(move || pipe.write_all(&data))
    });
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    ui::set_child(child.id());
    let waited = wait(&mut child, &program, spec.timeout);
    ui::set_child(0);
    let (status, timed_out) = waited?;
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
    let done = Done {
        stdout: text(out)?,
        stderr: text(err)?,
        status,
        program: program.clone(),
        timed_out,
    };
    if let Some(Err(e)) = writer
        .map(|w| w.join())
        .transpose()
        .map_err(|_| format!("feeding {program} panicked"))?
    {
        // A child that exited early without reading its input is reported through its own status.
        if done.ok() {
            return Err(format!("feeding {program}: {e}").into());
        }
    }
    Ok(done)
}

pub fn capture(cmd: &mut Command) -> Result<Done> {
    run(cmd, Spec::default())
}

/// Stdout of a command that must succeed.
pub fn stdout_of(cmd: &mut Command) -> Result<String> {
    Ok(capture(cmd)?.success()?.stdout)
}

/// Runs `cmd` with stdout in `dest` (and stderr in `errors` when given).
pub fn to_file(cmd: &mut Command, dest: &Path, errors: Option<&Path>) -> Result<Done> {
    run(
        cmd,
        Spec {
            stdout: Sink::File(dest),
            stderr: errors.map_or(Sink::Capture, Sink::File),
            ..Spec::default()
        },
    )
}
