//! The Preflight section: every read-only check of the run, in order, each with a mark. Unlike the
//! sections that write, it does not stop at the first failure: it runs every check it can, reports all
//! the failures together and only then stops. It writes nothing to the phone and no record.
use crate::adb::{self, Device, MIN_BATTERY};
use crate::config;
use crate::fail::{Fail, Result};
use crate::fastboot;
use crate::host;
use crate::images;
use crate::nura::{self, Build, Offer};
use crate::phone::{Serial, Slot};
use crate::render::{limit, plural, Mark};
use crate::ui::{self, Job};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

const FIX_TOOLS: &str = "[ ] Install or fix the program named above  [ ] Run again.";
const FIX_DISK: &str = "[ ] Free space in the cache directory, or move it  [ ] Run again.";
const FIX_DEVICE: &str = "[ ] Boot into Android  [ ] USB debugging on  [ ] Cable straight to this computer  [ ] Accept the RSA prompt  [ ] Run again.";
const FIX_PROP: &str = "[ ] Wait until Android is booted and unlocked  [ ] Run again.";
const FIX_ACCOUNTS: &str =
    "[ ] Settings > Passwords & accounts: sign out of every account  [ ] Run again.";
const FIX_BATTERY: &str = "[ ] Charge the phone  [ ] Run again.";
const FIX_NURA: &str =
    "[ ] Run again later  [ ] Still failing: Nura's server or its index may have changed; report it.";
const FIX_VAR: &str = "[ ] Flash nothing  [ ] Report this answer.";

struct Row {
    mark: Mark,
    label: String,
    detail: String,
    fix: &'static str,
}

pub struct Report {
    rows: Vec<Row>,
}

impl Report {
    pub fn new() -> Self {
        Report { rows: Vec::new() }
    }

    fn record(&mut self, mark: Mark, label: &str, detail: &str, fix: &'static str) {
        self.rows.push(Row {
            mark,
            label: label.to_string(),
            detail: detail.to_string(),
            fix,
        });
    }

    fn check<T>(
        &mut self,
        label: &str,
        fix: &'static str,
        work: impl FnOnce() -> Result<(T, String)>,
    ) -> Option<T> {
        self.checked(Job::new(label), label, fix, work)
    }

    /// A check whose wait is named up front, with what it waits for and how long.
    fn check_named<T>(
        &mut self,
        label: &str,
        naming: String,
        fix: &'static str,
        work: impl FnOnce() -> Result<(T, String)>,
    ) -> Option<T> {
        self.checked(Job::new(label).naming(naming), label, fix, work)
    }

    fn checked<T>(
        &mut self,
        job: Job,
        label: &str,
        fix: &'static str,
        work: impl FnOnce() -> Result<(T, String)>,
    ) -> Option<T> {
        let mut shown = String::new();
        let result = ui::run(job.explain(), || {
            let (value, detail) = work()?;
            shown = detail.clone();
            Ok((value, detail))
        });
        match result {
            Ok(value) => {
                self.record(Mark::Pass, label, &shown, fix);
                Some(value)
            }
            Err(e) => {
                self.record(Mark::Fail, label, &e.to_string(), fix);
                None
            }
        }
    }

    /// A check made of steps that print their own result lines; only a failure needs one here.
    fn check_shown<T>(
        &mut self,
        label: &str,
        fix: &'static str,
        result: Result<(T, String)>,
    ) -> Option<T> {
        match result {
            Ok((value, detail)) => {
                self.record(Mark::Pass, label, &detail, fix);
                Some(value)
            }
            Err(e) => {
                ui::say(&format!("  {} {label}: {e}", Mark::Fail.tag()));
                self.record(Mark::Fail, label, &e.to_string(), fix);
                None
            }
        }
    }

    fn skip(&mut self, label: &str, why: &str) {
        ui::skip(label, why);
        self.record(Mark::Skip, label, why, "");
    }

    fn count(&self, mark: Mark) -> usize {
        self.rows.iter().filter(|r| r.mark == mark).count()
    }

    /// Ends the Preflight: silent success, or every failure with what to do, and a stop.
    pub fn conclude(&self, run_again: &str) -> Result<()> {
        let failed = self.count(Mark::Fail);
        let skipped = self.count(Mark::Skip);
        if failed + skipped == 0 {
            ui::say(&format!(
                "  Preflight passed: {}",
                plural(self.rows.len(), "check")
            ));
            return Ok(());
        }
        ui::say(&format!(
            "\nNot ready: {} failed, {skipped} skipped.",
            plural(failed, "check")
        ));
        ui::say("What each one means and what to do:");
        for row in self.rows.iter().filter(|row| row.mark == Mark::Fail) {
            let first = row.detail.lines().next().unwrap_or("");
            ui::say(&format!("  - {}: {first}\n      {}", row.label, row.fix));
        }
        Err(Fail::Stop(format!(
            "Preflight not ready: {} failed, {skipped} skipped; nothing was written to the phone and no record was made. Next: fix what is listed above and run `{run_again}` again",
            plural(failed, "check")
        )))
    }
}

/// This computer: the programs and the disk space.
pub fn computer(r: &mut Report, cache: &Path) {
    r.check_named("tools", host::tools_naming(), FIX_TOOLS, host::check_tools);
    r.check("disk space", FIX_DISK, || host::check_disk(cache));
}

#[derive(Default)]
pub struct Facts {
    pub serial: Option<Serial>,
    pub slot: Option<Slot>,
}

fn same(value: String, want: &str, what: &str) -> Result<(String, String)> {
    if value == want {
        Ok((value.clone(), value))
    } else {
        Err(format!("the phone reports {what} {value}, adb listed {want}").into())
    }
}

fn wait_naming() -> Result<String> {
    Ok(format!(
        "waiting for the phone over adb{}",
        crate::render::up_to(config::wait_limit()?)
    ))
}

fn wait_for_device() -> Result<(Serial, String)> {
    let started = Instant::now();
    let wait = config::wait_limit()?.unwrap_or(Duration::MAX);
    loop {
        let list = adb::devices()?;
        match list.as_slice() {
            [Device { serial, state }] if state == "device" => {
                let serial = Serial::new(serial)?;
                return Ok((serial.clone(), serial.to_string()));
            }
            [] | [_] if started.elapsed() < wait => sleep(Duration::from_secs(2)),
            [] => {
                return Err(format!(
                    "no phone is visible over adb after waiting {}. {}",
                    limit(wait.as_secs()),
                    FIX_DEVICE
                )
                .into())
            }
            [Device { state, .. }] => {
                return Err(match state.as_str() {
                    "unauthorized" => "adb lists the phone as unauthorized: accept the RSA prompt on it".to_string(),
                    "offline" => "adb lists the phone as offline: unplug it, plug it in again and accept the RSA prompt".to_string(),
                    other => format!("adb lists the phone in state {other:?}, not `device`"),
                }
                .into())
            }
            many => {
                return Err(format!("{} adb devices are connected; connect only the phone", many.len()).into())
            }
        }
    }
}

/// The one phone over adb, waiting for it up to the limit.
pub fn find_phone(r: &mut Report) -> Option<Serial> {
    let waiting = wait_naming().unwrap_or_else(|e| e.to_string());
    r.check_named("exactly one phone", waiting, FIX_DEVICE, wait_for_device)
}

/// The phone on Android, read over adb.
pub fn adb_side(r: &mut Report, found: Option<Serial>) -> Facts {
    let mut facts = Facts::default();
    let Some(serial) = found else {
        r.skip("every other adb check", "it needs the phone over adb");
        return facts;
    };
    ui::say(&format!("  phone {serial}"));
    let get = |name: &str| adb::getprop(&serial, name);
    r.check("ro.serialno", FIX_PROP, || {
        get("ro.serialno").and_then(|v| same(v, serial.as_str(), "serial"))
    });
    facts.slot = r.check("ro.boot.slot_suffix", FIX_PROP, || {
        adb::slot_of(&serial).map(|s| (s, format!("slot {}", s.as_str())))
    });
    r.check("accounts signed in", FIX_ACCOUNTS, || {
        let n = adb::shell(&serial, "dumpsys account").and_then(|t| adb::parse_accounts(&t))?;
        if n > 0 {
            return Err(
                "accounts are signed in; remove them all in Settings, then run again".into(),
            );
        }
        Ok(((), "none signed in".to_string()))
    });
    r.check("battery", FIX_BATTERY, || {
        let level = adb::shell(&serial, "dumpsys battery").and_then(|t| adb::parse_battery(&t))?;
        if level < MIN_BATTERY {
            return Err(format!(
                "the battery is at {level}%; charge it to at least {MIN_BATTERY}% and run again"
            )
            .into());
        }
        Ok(((), format!("{level}%")))
    });
    facts.serial = Some(serial);
    facts
}

/// The phone that came back over adb after the bootloader check is the one that was checked.
pub fn same_phone(r: &mut Report, before: &Facts, after: &Facts) {
    r.check("the same phone as before", FIX_PROP, || {
        let same = before.serial == after.serial
            && before.slot == after.slot;
        if same {
            Ok(((), "same serial and slot".to_string()))
        } else {
            Err("the serial or slot differs from before the bootloader check; run again from the start".into())
        }
    });
}

/// Nura's builds for the phone the bootloader reported, from Nura's index fetched now.
pub fn builds(r: &mut Report, cache: &Path, vars: Option<&fastboot::Vars>) -> Option<Vec<Offer>> {
    let Some(vars) = vars else {
        r.skip(
            "Nura's builds for this phone",
            "it needs the bootloader's answers",
        );
        return None;
    };
    r.check("Nura's builds for this phone", FIX_NURA, || {
        let product = fastboot::product(vars)?;
        let device = nura::device_for(&product)?;
        let offers = nura::offers(&nura::fetch_index(cache)?, device)?;
        let shown = format!("{} interfaces for {device}", offers.len());
        Ok((offers, shown))
    })
}

/// Nura's images from the index: downloaded, checked against its hashes and unpacked.
pub fn images(r: &mut Report, build: &Build, cache: &Path) {
    ui::say(&format!("  Nura build: {}", build.label()));
    let result = images::nura(cache, build).map(|()| {
        (
            (),
            "downloads match Nura's index; images unpacked".to_string(),
        )
    });
    r.check_shown("the Nura images", FIX_NURA, result);
}

/// The bootloader's answers as checks of the report: `getvar all` once, then every variable the run
/// needs; all failures are reported together. Gives the answers when every check passed.
pub fn fastboot_side(r: &mut Report, serial: &Serial, slot: Slot) -> Option<fastboot::Vars> {
    let vars = r.check("getvar all", FIX_VAR, || {
        let vars = fastboot::read_all(serial)?;
        let shown = format!("{} variables", vars.len());
        Ok((vars, shown))
    })?;
    let mut all_passed = true;
    for c in fastboot::checks(serial, slot, &vars) {
        all_passed &= r
            .check(&c.label, FIX_VAR, || {
                let shown = (c.run)()?;
                Ok(((), shown))
            })
            .is_some();
    }
    all_passed.then_some(vars)
}
