//! What the owner reads. Every wait goes through one renderer: a naming line before it, a spinner
//! (terminal only) while it runs, and one finished line with its duration. Step headers carry their
//! total time, the run ends with `done in ...`, and the transcript gets the same lines plus a plain
//! progress line now and then for long steps, never spinner frames.
use crate::fail::Result;
use crate::phone::Serial;
use crate::render::{self, Mark, Meter};
use crate::{config, log, signals, steps};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

static ITEMS: AtomicU64 = AtomicU64::new(0);
static CHILD: AtomicU32 = AtomicU32::new(0);
static CONSOLE: Mutex<Console> = Mutex::new(Console::new());

const FRAME_EVERY: Duration = Duration::from_millis(200);

/// Where a running step's byte count comes from.
#[derive(Clone, Debug)]
pub enum Source {
    /// The size of a file the command is writing.
    FileSize(PathBuf),
    /// How far the running command has read into a file.
    ReadPos(PathBuf),
}

impl Source {
    pub fn read_pos(path: &Path) -> Source {
        Source::ReadPos(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
    }
}

/// What a running step measures; a total is given only where it is truly known.
#[derive(Clone, Debug)]
pub enum Gauge {
    None,
    Bytes {
        source: Source,
        total: Option<u64>,
    },
    Items {
        noun: &'static str,
        total: Option<u64>,
    },
}

impl Gauge {
    pub fn written(path: &Path, total: Option<u64>) -> Gauge {
        Gauge::Bytes {
            source: Source::FileSize(path.to_path_buf()),
            total,
        }
    }

    pub fn read(path: &Path, total: Option<u64>) -> Gauge {
        Gauge::Bytes {
            source: Source::read_pos(path),
            total,
        }
    }

    pub fn lines(noun: &'static str, total: Option<u64>) -> Gauge {
        Gauge::Items { noun, total }
    }
}

struct Active {
    label: String,
    sub: Option<String>,
    started: Instant,
    gauge: Gauge,
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
    cache: Option<PathBuf>,
    serial: Option<Serial>,
    wrote: bool,
    noted: bool,
    cur: Option<&'static steps::Step>,
    asking: Option<String>,
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
            cache: None,
            serial: None,
            wrote: false,
            noted: false,
            cur: None,
            asking: None,
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
    thread::spawn(|| {
        // The signals belong to the main thread, which may hold them; this one must never take one.
        signals::block_in_this_thread();
        loop {
            sleep(FRAME_EVERY);
            beat();
        }
    });
}

fn bytes_of(source: &Source) -> Option<u64> {
    match source {
        Source::FileSize(path) => fs::metadata(path).ok().map(|m| m.len()),
        Source::ReadPos(path) => read_pos(CHILD.load(Ordering::Relaxed), path),
    }
}

fn read_pos(pid: u32, path: &Path) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    for entry in fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
        if fs::read_link(entry.path()).ok().as_deref() == Some(path) {
            let info = fs::read_to_string(format!(
                "/proc/{pid}/fdinfo/{}",
                entry.file_name().to_string_lossy()
            ))
            .ok()?;
            return parse_pos(&info);
        }
    }
    None
}

/// The `pos:` line of /proc/<pid>/fdinfo/<fd>.
pub fn parse_pos(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("pos:")?.trim().parse().ok())
}

fn meter_of(active: &Active, with_bar: bool) -> String {
    let elapsed = ms(active.started);
    let meter = match &active.gauge {
        Gauge::None => Meter::None,
        Gauge::Bytes { source, total } => Meter::Bytes {
            done: bytes_of(source).unwrap_or(0),
            total: *total,
        },
        Gauge::Items { noun, total } => Meter::Items {
            done: ITEMS.load(Ordering::Relaxed),
            total: *total,
            noun,
        },
    };
    render::meter_text(meter, elapsed, with_bar)
}

fn label_of(active: &Active) -> String {
    match &active.sub {
        Some(sub) => format!("{}: {sub}", active.label),
        None => active.label.clone(),
    }
}

/// One heartbeat: notices Ctrl-C, redraws the spinner, writes a progress line to the transcript.
fn beat() {
    if signals::interrupted() && !signals::in_group() {
        match signals::copy_phase() {
            signals::Copy::Off => crate::interrupted(),
            signals::Copy::Session => kill_child(),
            signals::Copy::Stopping => {}
        }
    }
    let mut c = lock();
    if signals::in_group() && !c.noted && signals::pending() {
        c.noted = true;
        out(&mut c, "  interrupt noted; finishing this group first");
    }
    update_title(&mut c);
    let tty = c.tty;
    let Some(active) = c.active.as_ref() else {
        return;
    };
    let elapsed = ms(active.started);
    let label = label_of(active);
    let line =
        tty.then(|| render::spinner(c.tick, &label, &meter_of(active, true), elapsed, columns()));
    let due = active.last_log.elapsed() >= config::progress_every();
    let progress = due.then(|| render::progress_line(&label, &meter_of(active, false), elapsed));
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

fn start(c: &mut Console, label: &str, gauge: Gauge) {
    ITEMS.store(0, Ordering::Relaxed);
    c.active = Some(Active {
        label: label.to_string(),
        sub: None,
        started: Instant::now(),
        gauge,
        last_log: Instant::now(),
    });
}

fn finish(c: &mut Console) -> u64 {
    clear_live(c);
    c.active.take().map_or(0, |a| ms(a.started))
}

pub fn add_items(n: u64) {
    ITEMS.fetch_add(n, Ordering::Relaxed);
}

pub fn reset_counters() {
    ITEMS.store(0, Ordering::Relaxed);
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

/// Replaces the running step's gauge, for work that moves on to a measurable part.
pub fn set_gauge(gauge: Gauge) {
    if let Some(a) = lock().active.as_mut() {
        a.gauge = gauge;
    }
}

/// A command's own line to show above the spinner, such as fastboot's progress.
pub fn relay(line: &str) {
    let mut c = lock();
    let line = line.trim_end();
    if line.is_empty() {
        return;
    }
    if let Some(a) = c.active.as_mut() {
        a.sub = Some(line.trim().to_string());
    }
    out(&mut c, &format!("    {line}"));
}

/// Marks a stretch of work inside the running step (or a step of its own when none runs).
pub struct Busy {
    created: bool,
    replace: bool,
    previous: Option<(Option<String>, Gauge)>,
}

fn enter(what: &str, gauge: Gauge, replace: bool) -> Busy {
    let mut c = lock();
    match c.active.as_mut() {
        Some(a) if replace => Busy {
            created: false,
            replace,
            previous: Some((
                a.sub.replace(what.to_string()),
                std::mem::replace(&mut a.gauge, gauge),
            )),
        },
        Some(_) => Busy {
            created: false,
            replace,
            previous: None,
        },
        None => {
            start(&mut c, what, gauge);
            Busy {
                created: true,
                replace,
                previous: None,
            }
        }
    }
}

/// Names `what` and shows its `gauge` for as long as the guard lives.
pub fn busy(what: &str, gauge: Gauge) -> Busy {
    enter(what, gauge, true)
}

/// Makes sure something is shown while a command runs, without disturbing a step that already is.
pub fn ensure(what: &str) -> Busy {
    enter(what, Gauge::None, false)
}

impl Drop for Busy {
    fn drop(&mut self) {
        let mut c = lock();
        if self.created {
            finish(&mut c);
        } else if let (true, Some((sub, gauge)), Some(a)) =
            (self.replace, self.previous.take(), c.active.as_mut())
        {
            a.sub = sub;
            a.gauge = gauge;
        }
    }
}

/// One check, command or wait: `label` names it in the result line.
pub struct Job<'a> {
    label: &'a str,
    naming: Option<String>,
    gauge: Gauge,
    explain: bool,
}

impl<'a> Job<'a> {
    pub fn new(label: &'a str) -> Self {
        Job {
            label,
            naming: None,
            gauge: Gauge::None,
            explain: false,
        }
    }

    /// Says what is about to happen and how big it is; on a terminal this line is printed only for
    /// jobs that have one, since the spinner already names the quick ones.
    pub fn naming(mut self, text: impl Into<String>) -> Self {
        self.naming = Some(text.into());
        self
    }

    pub fn gauge(mut self, gauge: Gauge) -> Self {
        self.gauge = gauge;
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
        start(&mut c, job.label, job.gauge);
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

/// The question the run is waiting on, for the terminal title; `None` when it is answered.
pub fn asking(prompt: Option<&str>) {
    let mut c = lock();
    c.asking = prompt.map(|p| p.split('`').nth(1).unwrap_or("an answer").to_string());
    c.title_at = None;
    update_title(&mut c);
}

/// The share done of the running work, only where the screen shows a bar for it.
fn bar_percent(active: &Active) -> Option<u64> {
    let (done, total) = match &active.gauge {
        Gauge::Bytes {
            source,
            total: Some(t),
        } => (bytes_of(source).unwrap_or(0), *t),
        Gauge::Items { total: Some(t), .. } => (ITEMS.load(Ordering::Relaxed), *t),
        _ => return None,
    };
    (done <= total).then(|| render::percent(done, total))
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
    let asking = c.asking.clone();
    let text = if let Some(what) = asking.as_deref() {
        render::title(
            &render::Title::Waiting {
                n,
                total,
                what,
                step_ms,
                run_ms,
            },
            c.title_tick,
        )
    } else if step.actor == steps::Actor::You {
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
                pct: c.active.as_ref().and_then(bar_percent),
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

pub fn set_cache(cache: &Path) {
    lock().cache = Some(cache.to_path_buf());
}

pub fn set_phone(serial: &Serial) {
    lock().serial = Some(serial.clone());
}

/// A write to the phone has begun, so a later stop cannot say nothing was written.
pub fn note_written() {
    lock().wrote = true;
}

pub struct Context {
    pub cache: Option<PathBuf>,
    pub serial: Option<Serial>,
    pub wrote: bool,
}

pub fn context() -> Context {
    let c = lock();
    Context {
        cache: c.cache.clone(),
        serial: c.serial.clone(),
        wrote: c.wrote,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_position_is_the_pos_line_of_fdinfo() {
        assert_eq!(
            parse_pos("pos:\t1048576\nflags:\t0100000\nmnt_id:\t29\n"),
            Some(1_048_576)
        );
        assert_eq!(parse_pos("flags:\t0\n"), None);
        assert_eq!(parse_pos("pos:\tmany\n"), None);
    }
}
