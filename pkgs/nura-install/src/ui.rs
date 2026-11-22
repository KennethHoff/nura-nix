//! What the owner reads. Every wait goes through one renderer: a naming line before it, a spinner
//! (terminal only) while it runs, and one finished line with its duration. Step headers carry their
//! total time, the run ends with `done in ...`, and the transcript gets the same lines plus a plain
//! progress line now and then for long steps, never spinner frames.
use crate::fail::Result;
use crate::render::{self, Mark};
use crate::{config, log, signals, steps};
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

static CHILD: AtomicU32 = AtomicU32::new(0);
static CONSOLE: Mutex<Console> = Mutex::new(Console::new());

const FRAME_EVERY: Duration = Duration::from_millis(200);

struct Active {
    label: String,
    started: Instant,
    last_log: Instant,
}

struct Console {
    tty: bool,
    run_started: Option<Instant>,
    phase: Option<(String, Instant)>,
    live: bool,
    tick: usize,
    active: Option<Active>,
    step: u32,
    total: u32,
    cur: Option<&'static steps::Step>,
    title_at: Option<Instant>,
    title_tick: usize,
}

impl Console {
    const fn new() -> Self {
        Console {
            tty: false,
            run_started: None,
            phase: None,
            live: false,
            tick: 0,
            active: None,
            step: 0,
            total: 0,
            cur: None,
            title_at: None,
            title_tick: 0,
        }
    }
}

fn lock() -> MutexGuard<'static, Console> {
    CONSOLE.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Writes to the screen and ignores a failure: a hung-up or closed terminal must not panic the run
/// (as `print!` would) while a group of writes is half done. The transcript still has every line.
pub fn put(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
}

fn clear_live(c: &mut Console) {
    if c.live {
        put(render::clear());
        c.live = false;
    }
}

fn out(c: &mut Console, line: &str) {
    clear_live(c);
    put(&format!("{line}\n"));
    log::line(line);
}

fn columns() -> usize {
    // SAFETY: winsize is plain data and TIOCGWINSZ only fills the struct we pass.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0 {
        usize::from(size.ws_col)
    } else {
        80
    }
}

/// Detects the terminal, starts the heartbeat thread and the run clock.
pub fn init() {
    // SAFETY: isatty only inspects the descriptor number.
    let tty = unsafe { libc::isatty(1) == 1 };
    {
        let mut c = lock();
        c.tty = tty;
        c.run_started = Some(Instant::now());
    }
    thread::spawn(|| loop {
        sleep(FRAME_EVERY);
        beat();
    });
}

/// One heartbeat: notices Ctrl-C, redraws the spinner, writes a progress line to the transcript.
fn beat() {
    if signals::interrupted() {
        crate::interrupted();
    }
    let mut c = lock();
    update_title(&mut c);
    let tty = c.tty;
    let Some(active) = c.active.as_ref() else {
        return;
    };
    let elapsed = ms(active.started);
    let label = active.label.clone();
    let line = tty.then(|| render::spinner(c.tick, &label, elapsed, columns()));
    let due = active.last_log.elapsed() >= config::progress_every();
    let progress = due.then(|| render::progress_line(&label, elapsed));
    if let Some(line) = line {
        put(&line);
        c.live = true;
        c.tick += 1;
    }
    if let Some(line) = progress {
        log::line(&line);
        if let Some(a) = c.active.as_mut() {
            a.last_log = Instant::now();
        }
    }
}

fn start(c: &mut Console, label: &str) {
    c.active = Some(Active {
        label: label.to_string(),
        started: Instant::now(),
        last_log: Instant::now(),
    });
}

fn finish(c: &mut Console) -> u64 {
    clear_live(c);
    c.active.take().map_or(0, |a| ms(a.started))
}

/// The process the running step is waiting for, for gauges that read its file positions.
pub fn set_child(pid: u32) {
    CHILD.store(pid, Ordering::Relaxed);
}

pub fn kill_child() {
    let pid = CHILD.load(Ordering::Relaxed);
    if pid != 0 {
        // SAFETY: kill takes plain integers; a pid that already exited just fails.
        let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
}

/// Shows `what` while a command runs, unless a step already shows something.
pub struct Busy {
    created: bool,
}

pub fn ensure(what: &str) -> Busy {
    let mut c = lock();
    if c.active.is_some() {
        return Busy { created: false };
    }
    start(&mut c, what);
    Busy { created: true }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.created {
            finish(&mut lock());
        }
    }
}

/// One check, command or wait: `label` names it in the result line.
pub struct Job<'a> {
    label: &'a str,
    naming: Option<String>,
    explain: bool,
}

impl<'a> Job<'a> {
    pub fn new(label: &'a str) -> Self {
        Job {
            label,
            naming: None,
            explain: false,
        }
    }

    /// Says what is about to happen and how big it is; on a terminal this line is printed only for
    /// jobs that have one, since the spinner already names the quick ones.
    pub fn naming(mut self, text: impl Into<String>) -> Self {
        self.naming = Some(text.into());
        self
    }

    /// A failure prints its reason in the result line (the pre-flight goes on after it); otherwise
    /// the error is reported once by the caller.
    pub fn explain(mut self) -> Self {
        self.explain = true;
        self
    }
}

/// Runs `work` as a job and prints its result line; the error is returned after it was shown.
pub fn run<T>(job: Job, work: impl FnOnce() -> Result<(T, String)>) -> Result<T> {
    {
        let mut c = lock();
        if !c.tty || job.naming.is_some() {
            let naming = job
                .naming
                .clone()
                .unwrap_or_else(|| format!("checking {}", job.label));
            out(&mut c, &format!("  ... {naming}"));
        }
        start(&mut c, job.label);
    }
    let result = work();
    let mut c = lock();
    let took = finish(&mut c);
    match result {
        Ok((value, detail)) => {
            out(
                &mut c,
                &render::result(Mark::Pass, job.label, &detail, took),
            );
            Ok(value)
        }
        Err(e) => {
            let why = if job.explain {
                e.to_string()
            } else {
                String::new()
            };
            out(&mut c, &render::result(Mark::Fail, job.label, &why, took));
            Err(e)
        }
    }
}

/// A check that did not run.
pub fn skip(label: &str, why: &str) {
    out(&mut lock(), &render::skipped(label, why));
}

fn end_phase(c: &mut Console) {
    if let Some((name, started)) = c.phase.take() {
        out(c, &render::finished(&format!("{name} done"), ms(started)));
    }
}

/// A section of the run: a header line of its own before its first step.
pub fn section(name: &str) {
    let mut c = lock();
    end_phase(&mut c);
    out(&mut c, &format!("\n=== {name}"));
}

/// Starts numbering: the steps that follow are "Step 1 of `total`" and on.
pub fn plan(total: u32) {
    let mut c = lock();
    c.step = 0;
    c.total = total;
}

pub fn step(step: &'static steps::Step) {
    let mut c = lock();
    end_phase(&mut c);
    c.step += 1;
    c.cur = Some(step);
    c.title_at = None;
    let (n, total) = (c.step, c.total);
    out(&mut c, &format!("\n{}", steps::header(n, total, step)));
    for line in steps::block(step) {
        out(&mut c, &line);
    }
    c.phase = Some((format!("Step {n} of {total}"), Instant::now()));
}

pub fn say(line: &str) {
    out(&mut lock(), line);
}

/// Ends the current step with its duration, before a failure is reported under it.
pub fn end_step() {
    let mut c = lock();
    end_phase(&mut c);
}

/// Sets the terminal title at most once a second, on a terminal only.
fn update_title(c: &mut Console) {
    if !c.tty
        || c.title_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
    {
        return;
    }
    let (Some(step), Some((_, step_started)), Some(run_started)) =
        (c.cur, c.phase.as_ref(), c.run_started)
    else {
        return;
    };
    let (n, total) = (c.step, c.total);
    let (step_ms, run_ms) = (ms(*step_started), ms(run_started));
    let text = if step.actor == steps::Actor::You {
        render::title(
            &render::Title::Waiting {
                n,
                total,
                what: step.short,
                step_ms,
                run_ms,
            },
            c.title_tick,
        )
    } else {
        render::title(
            &render::Title::Working {
                n,
                total,
                label: step.title,
                step_ms,
                run_ms,
            },
            c.title_tick,
        )
    };
    put(&render::set_title(&text));
    c.title_tick += 1;
    c.title_at = Some(Instant::now());
}

/// The last line of every run.
pub fn finish_run(outcome: &str) {
    let mut c = lock();
    end_phase(&mut c);
    let total = c.run_started.map_or(0, ms);
    if c.tty {
        let state = if outcome == "finished" || outcome == "nothing to do" {
            render::Title::Done { run_ms: total }
        } else {
            render::Title::Stopped {
                n: c.step,
                total: c.total,
                run_ms: total,
            }
        };
        put(&render::set_title(&render::title(&state, 0)));
    }
    out(&mut c, &render::run_end(total, outcome));
}

/// Takes the spinner off the screen before anything else is printed.
pub fn stop_live() {
    let mut c = lock();
    finish(&mut c);
}
