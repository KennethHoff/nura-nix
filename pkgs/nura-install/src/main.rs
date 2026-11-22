mod adb;
mod cmd;
mod config;
mod copy;
mod fail;
mod fastboot;
mod group;
mod host;
mod images;
mod log;
mod nura;
mod phone;
mod preflight;
mod prompt;
mod render;
mod route;
mod signals;
mod state;
mod steps;
mod ui;

use copy::{Taken, Wait};
use fail::{Fail, Result};
use group::{execute, Group, Origin, Phone};
use nura::Offer;
use phone::Serial;
use preflight::{Facts, Found, Report};
use prompt::Entry;
use route::Route;
use state::{CopyRecord, Record, State};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "Usage: nura-install

Takes no arguments: the run follows from the phone's state and its record.

  Android over adb, no record     install Nura: one run in four sections (Preflight, Prepare,
                                  Install, Finish)
  Nura running, not yet copied    only the copy of the per-phone partitions
  Nura installed, phone in fastboot     reinstall Nura
  Nura half installed, phone in fastboot  finish the install
  -h, --help                      this text

Exit codes:
  0  finished
  1  stopped or failed (the Preflight: at least one check failed, it runs them all);
     the message says what state the phone and its record are in
  2  usage error
  3  nothing to do for the phone's current state";

const WARNINGS: &str = "\
Rules:
  - never relock the bootloader
  - never run `qbootctl -s` or switch slots, on either system
  - Nura not booting after 1-2 tries: fastboot (Volume Down + Power), then `nura-install` finishes or repeats the install";

/// `Ok(true)` for the help text; the tool takes no other argument.
fn parse_args(mut args: impl Iterator<Item = String>) -> std::result::Result<bool, String> {
    match args.next().as_deref() {
        None => Ok(false),
        Some("-h" | "--help") if args.next().is_none() => Ok(true),
        Some(other) => Err(format!(
            "unexpected argument {other:?}; nura-install takes none"
        )),
    }
}

/// Ctrl-C, hang-up or terminate outside a write group: says what state the phone and the record are in, then stops.
pub fn interrupted() -> ! {
    prompt::restore_terminal();
    ui::stop_live();
    ui::kill_child();
    let ctx = ui::context();
    let written = if ctx.wrote {
        "Some writes of this run may already be on the phone."
    } else {
        "Nothing was written to the phone."
    };
    let record = match (&ctx.cache, &ctx.serial) {
        (Some(cache), Some(serial)) => state::describe(cache, serial),
        _ => "No record was changed.".to_string(),
    };
    ui::end_step();
    ui::say(&format!("\ninterrupted: {written} {record}"));
    if let Some(note) = copy::unstopped_sshd() {
        ui::say(&note);
    }
    log::line("outcome: interrupted");
    finish_output("interrupted");
    std::process::exit(i32::from(signals::exit_code()))
}

/// The line every summary carries about the backup; a missing one is said loudly.
fn backup_note(recorded: &str) -> String {
    if recorded == adb::NO_BACKUP {
        "NO BACKUP WAS TAKEN: Android and its data are gone for good.".to_string()
    } else {
        format!("The backup of Android stays under {recorded}.")
    }
}

fn finish_output(outcome: &str) {
    ui::finish_run(outcome);
}

fn main() -> ExitCode {
    if let Some(code) = copy::askpass() {
        return code;
    }
    match parse_args(std::env::args().skip(1)) {
        Ok(false) => {}
        Ok(true) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("nura-install: {msg}\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    if let Err(e) = signals::install_interrupt() {
        eprintln!("nura-install: {e}");
        return ExitCode::from(1);
    }
    ui::init();
    let outcome = run();
    ui::end_step();
    let (code, label) = match &outcome {
        Ok(()) => {
            log::line("outcome: finished");
            (0, "finished")
        }
        Err(fail) => {
            log::line(&format!("outcome: {fail}"));
            let label = match fail {
                Fail::Stop(msg) => {
                    eprintln!("\nnura-install: {msg}");
                    "stopped"
                }
                Fail::NothingToDo(msg) => {
                    println!("\nNothing to do: {msg}");
                    "nothing to do"
                }
            };
            if matches!(fail, Fail::Stop(_)) {
                let ctx = ui::context();
                if let (Some(cache), Some(serial)) = (&ctx.cache, &ctx.serial) {
                    eprintln!("{}", state::describe(cache, serial));
                }
            }
            (fail.exit_code(), label)
        }
    };
    finish_output(label);
    ExitCode::from(code)
}

enum CopyOutcome {
    Taken(Taken),
    Missing { why: String, skipped: bool },
}

fn run() -> Result<()> {
    let euid = host::harden_process()?;
    let cache = config::cache_dir()?;
    host::ensure_cache(&cache, euid)?;
    ui::set_cache(&cache);
    log::open(&cache, &host::timestamp()?, "run")?;
    let _lock = host::lock(&cache)?;
    match route::decide(&cache) {
        Ok(route) => {
            prompt::require_terminal()?;
            match route {
                Route::Fresh { wait } => install_fresh(&cache, wait),
                Route::Nura { serial, record } => nura_from_fastboot(&cache, serial, record),
                Route::CopyOnly { serial, record } => copy_only(&cache, serial, record),
            }
        }
        Err(Fail::Stop(msg)) => unreadable_surroundings(&cache, msg),
        Err(other) => Err(other),
    }
}

/// The computer could not tell where the phone is: say so after checking what the run needs, in case
/// that is the cause.
fn unreadable_surroundings(cache: &Path, msg: String) -> Result<()> {
    ui::plan(1);
    ui::section("Preflight");
    ui::step(&steps::CHECK_COMPUTER);
    let mut report = Report::new();
    preflight::computer(&mut report, cache);
    report.conclude("nura-install")?;
    Err(Fail::Stop(msg))
}

/// Steps of the Nura run from fastboot.
fn nura_steps(record: &Record) -> u32 {
    10 + 2 * u32::from(record.copy.is_none())
}

fn the_run_lost_track<T>() -> Result<T> {
    Err("every check passed, but a value the run needs is missing; this is a bug in nura-install, nothing was written".into())
}

/// The owner's pick among the interfaces Nura builds for the phone; only its images are downloaded.
fn choose_interface(offers: Vec<Offer>) -> Result<Offer> {
    ui::step(&steps::CHOOSE_UI);
    let entries: Vec<Entry> = offers
        .iter()
        .map(|o| Entry {
            word: o.word.clone(),
            label: o.label.clone(),
            about: if o.about.is_empty() {
                o.build.label()
            } else {
                o.about.clone()
            },
        })
        .collect();
    let picked = prompt::menu(&entries)?.ok_or("stopped; nothing was written to the phone")?;
    offers.into_iter().nth(picked).ok_or_else(|| {
        "the chosen interface is not in the list; this is a bug in nura-install".into()
    })
}

fn install_fresh(cache: &Path, wait: bool) -> Result<()> {
    ui::plan(19 + u32::from(wait));
    ui::section("Preflight");
    ui::step(&steps::CHECK_COMPUTER);
    let mut report = Report::new();
    preflight::computer(&mut report, cache);
    let mut found_phone = None;
    if wait {
        ui::step(&steps::CONNECT);
        found_phone = preflight::find_phone(&mut report);
    }
    ui::step(&steps::CHECK_ADB);
    if !wait {
        found_phone = preflight::find_phone(&mut report);
    }
    let facts = preflight::adb_side(&mut report, cache, found_phone)?;
    report.conclude("nura-install")?;
    let Facts {
        serial: Some(serial),
        slot: Some(slot),
        ..
    } = &facts
    else {
        return the_run_lost_track();
    };
    let (serial, slot) = (serial.clone(), *slot);

    ui::step(&steps::TO_FASTBOOT);
    fastboot::wait_for_phone()?;
    ui::step(&steps::CHECK_BOOTLOADER);
    let mut bootloader = Report::new();
    let early = preflight::fastboot_side(&mut bootloader, &serial, slot);
    ui::step(&steps::FIND_BUILDS);
    let mut finding = Report::new();
    let offers = preflight::builds(&mut finding, cache, &serial, early.as_ref());
    bootloader.conclude("nura-install")?;
    finding.conclude("nura-install")?;
    let (Some(_), Some(offers)) = (early, offers) else {
        return the_run_lost_track();
    };
    let offer = choose_interface(offers)?;

    ui::step(&steps::RESTART_TO_ANDROID);
    let mut report = Report::new();
    let found_phone = preflight::find_phone(&mut report);
    let again = preflight::adb_side(&mut report, cache, found_phone)?;
    preflight::same_phone(&mut report, &facts, &again);
    ui::step(&steps::FIND_IMAGES);
    let found = preflight::images(&mut report, &offer.build, cache);
    report.conclude("nura-install")?;
    let Some(found) = found else {
        return the_run_lost_track();
    };

    ui::section("Prepare");
    ui::step(&steps::CONFIRM);
    adb::confirm_losses()?;
    let take_backup = adb::choose_backup()?;
    ui::step(&steps::BACKUP);
    let backup = if take_backup {
        Some(adb::backup(cache, &serial)?)
    } else {
        ui::say("  skipped: no backup is taken");
        None
    };
    ui::step(&steps::TO_FASTBOOT);
    fastboot::wait_for_phone()?;
    ui::step(&steps::CHECK_FASTBOOT);
    fastboot::check_phone(&serial, slot)?;
    let Found {
        nura,
        nura_identity,
    } = found;
    let install = Group::install(&nura, Origin::Fresh)?;
    group::images_fit(&serial, slot, &install)?;
    let record = Record {
        state: State::NuraInProgress,
        slot,
        backup: backup.map_or_else(|| adb::NO_BACKUP.to_string(), |p| p.display().to_string()),
        nura: nura_identity,
        copy: None,
    };
    ui::say(&format!(
        "  all fastboot checks passed (slot {})",
        record.slot.as_str()
    ));
    let phone = Phone {
        serial,
        cache: cache.to_path_buf(),
        record,
    };

    ui::section("Install");
    ui::step(&steps::APPROVE_INSTALL);
    ui::say(WARNINGS);
    execute(&phone, install, &steps::INSTALL)?;
    copy::forget_host_key(&phone.cache, &phone.serial)?;

    ui::section("Finish");
    ui::step(&steps::APPROVE_REBOOT);
    execute(&phone, Group::reboot()?, &steps::REBOOT)?;
    finish_nura(&phone, true)
}

fn nura_from_fastboot(cache: &Path, serial: Serial, record: Record) -> Result<()> {
    let copy_pending = record.copy.is_none();
    ui::set_phone(&serial);
    ui::plan(nura_steps(&record));
    ui::section("Preflight");
    ui::say(if record.state == State::NuraInstalled {
        "  Nura is installed: this run reinstalls it."
    } else {
        "  Nura is half installed: this run finishes the install."
    });
    ui::step(&steps::CHECK_RECORD);
    let mut report = Report::new();
    preflight::computer(&mut report, cache);
    preflight::record(&mut report, &serial, &record);
    ui::step(&steps::CHECK_FASTBOOT_RECORDED);
    let src = preflight::fastboot_side(&mut report, &serial, record.slot);
    ui::step(&steps::FIND_BUILDS);
    let offers = preflight::builds(&mut report, cache, &serial, src.as_ref());
    report.conclude("nura-install")?;
    let Some(offers) = offers else {
        return the_run_lost_track();
    };
    let offer = choose_interface(offers)?;
    ui::step(&steps::FIND_NURA);
    let mut report = Report::new();
    let nura = preflight::nura_images(&mut report, &offer.build, cache);
    report.conclude("nura-install")?;
    let Some((nura, identity)) = nura else {
        return the_run_lost_track();
    };
    let reinstall = record.state == State::NuraInstalled;
    let phone = Phone {
        serial,
        cache: cache.to_path_buf(),
        record: record.with_nura(identity),
    };

    ui::section("Install");
    let (group, approve, apply) = if reinstall {
        (
            Group::reinstall(&nura)?,
            &steps::APPROVE_REINSTALL,
            &steps::REINSTALL,
        )
    } else {
        (
            Group::install(&nura, Origin::Recorded)?,
            &steps::APPROVE_INSTALL,
            &steps::INSTALL,
        )
    };
    ui::step(approve);
    ui::say(WARNINGS);
    execute(&phone, group, apply)?;
    copy::forget_host_key(&phone.cache, &phone.serial)?;

    ui::section("Finish");
    ui::step(&steps::APPROVE_REBOOT);
    execute(&phone, Group::reboot()?, &steps::REBOOT)?;
    finish_nura(&phone, copy_pending)
}

fn copy_only(cache: &Path, serial: Serial, record: Record) -> Result<()> {
    ui::set_phone(&serial);
    ui::plan(4);
    ui::section("Preflight");
    ui::step(&steps::CHECK_RECORD);
    let mut report = Report::new();
    preflight::computer(&mut report, cache);
    preflight::record(&mut report, &serial, &record);
    report.conclude("nura-install")?;
    let phone = Phone {
        serial,
        cache: cache.to_path_buf(),
        record,
    };
    ui::section("Finish");
    match copy_steps(&phone)? {
        CopyOutcome::Taken(taken) => {
            ui::step(&steps::SUMMARY);
            ui::say(&format!(
                "The per-phone partitions are copied to {}.",
                taken.dir.display()
            ));
            Ok(())
        }
        CopyOutcome::Missing { why, skipped } => {
            ui::step(&steps::SUMMARY);
            ui::say(&format!("The per-phone partitions are NOT copied: {why}."));
            ui::say(MISSING_COPY);
            if skipped {
                Err(Fail::NothingToDo(
                    "the copy was skipped at your request, so nothing was done".to_string(),
                ))
            } else {
                Err(Fail::Stop(format!("the copy was not taken: {why}")))
            }
        }
    }
}

const MISSING_COPY: &str = "\
To copy later:
  [ ] On Nura (user / 147147): `sudo systemctl start sshd`
  [ ] Run `nura-install` (does only the copy)";

/// The owner starts sshd, then the tool copies; a skipped or failed copy does not fail the install.
fn copy_steps(phone: &Phone) -> Result<CopyOutcome> {
    ui::step(&steps::START_SSHD);
    let waited = copy::wait_for_sshd();
    copy::sshd_may_run(matches!(waited, Ok(Wait::Ready)));
    ui::step(&steps::COPY);
    let (why, skipped) = match waited {
        Ok(Wait::Ready) => {
            let (result, sshd) = copy::run(&phone.cache, &phone.serial);
            ui::say(&format!("  {sshd}"));
            match result {
                Ok(taken) => {
                    let record =
                        phone
                            .record
                            .with_state(State::NuraInstalled)
                            .with_copy(CopyRecord {
                                path: taken.dir.display().to_string(),
                                time: taken.time.clone(),
                            });
                    state::write(&phone.cache, &phone.serial, &record)?;
                    return Ok(CopyOutcome::Taken(taken));
                }
                Err(e) => {
                    ui::say(&format!("  the copy failed: {e}"));
                    (e.to_string(), false)
                }
            }
        }
        Ok(Wait::Skipped) => {
            ui::say("  nothing is copied: skipped at your request");
            ("skipped at your request".to_string(), true)
        }
        Err(e) => {
            ui::say(&format!("  nothing is copied: {e}"));
            (e.to_string(), false)
        }
    };
    Ok(CopyOutcome::Missing { why, skipped })
}

fn finish_nura(phone: &Phone, copy_pending: bool) -> Result<()> {
    let outcome = if copy_pending {
        Some(copy_steps(phone)?)
    } else {
        None
    };
    ui::step(&steps::SUMMARY);
    ui::say("Nura is booting. Log in as `user` with password 147147.");
    ui::say("  - kernel updates only while the phone is charged and plugged in");
    ui::say(&backup_note(&phone.record.backup));
    match (outcome, &phone.record.copy) {
        (Some(CopyOutcome::Taken(taken)), _) => ui::say(&format!(
            "The per-phone partitions are copied to {}.",
            taken.dir.display()
        )),
        (Some(CopyOutcome::Missing { why, .. }), _) => {
            ui::say(&format!("The per-phone partitions are NOT copied: {why}."));
            ui::say(MISSING_COPY);
        }
        (None, Some(copy)) => ui::say(&format!(
            "The per-phone partitions were copied earlier, to {} ({}).",
            copy.path, copy.time
        )),
        (None, None) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> std::result::Result<bool, String> {
        parse_args(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_argument_runs_and_only_help_is_accepted() {
        assert_eq!(args(&[]), Ok(false));
        assert_eq!(args(&["--help"]), Ok(true));
        assert_eq!(args(&["-h"]), Ok(true));
        assert!(args(&["--os", "nura"]).is_err());
        assert!(args(&["nura"]).is_err());
        assert!(args(&["--help", "--other"]).is_err());
    }
}
