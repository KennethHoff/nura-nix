mod adb;
mod cmd;
mod config;
mod fail;
mod fastboot;
mod host;
mod log;
mod phone;
mod preflight;
mod render;
mod signals;
mod steps;
mod ui;

use fail::{Fail, Result};
use preflight::{Facts, Report};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "Usage: nura-install

Takes no arguments: it reads the phone over adb and fastboot and checks that it can take Nura.
Nothing is written to the phone.

  Android over adb                check the phone and its bootloader, in two sections (Preflight,
                                  Finish)
  -h, --help                      this text

Exit codes:
  0  finished
  1  stopped or failed (the Preflight: at least one check failed, it runs them all)
  2  usage error";

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

/// Ctrl-C, hang-up or terminate: says that nothing was written to the phone, then stops.
pub fn interrupted() -> ! {
    ui::stop_live();
    ui::kill_child();
    ui::end_step();
    ui::say("\ninterrupted: Nothing was written to the phone.");
    log::line("outcome: interrupted");
    finish_output("interrupted");
    std::process::exit(i32::from(signals::exit_code()))
}

fn finish_output(outcome: &str) {
    ui::finish_run(outcome);
}

fn main() -> ExitCode {
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
            };
            (fail.exit_code(), label)
        }
    };
    finish_output(label);
    ExitCode::from(code)
}

fn run() -> Result<()> {
    let euid = host::harden_process()?;
    let cache = config::cache_dir()?;
    host::ensure_cache(&cache, euid)?;
    log::open(&cache, &host::timestamp()?, "run")?;
    let _lock = host::lock(&cache)?;
    match adb::devices() {
        Ok(devices) => read_phone(&cache, devices.is_empty()),
        Err(Fail::Stop(msg)) => unreadable_surroundings(&cache, msg),
    }
}

/// The computer could not tell whether a phone is connected: say so after checking what the run needs, in
/// case that is the cause.
fn unreadable_surroundings(cache: &Path, msg: String) -> Result<()> {
    ui::plan(1);
    ui::section("Preflight");
    ui::step(&steps::CHECK_COMPUTER);
    let mut report = Report::new();
    preflight::computer(&mut report, cache);
    report.conclude("nura-install")?;
    Err(Fail::Stop(msg))
}

fn the_run_lost_track<T>() -> Result<T> {
    Err("every check passed, but a value the run needs is missing; this is a bug in nura-install, nothing was written".into())
}

fn read_phone(cache: &Path, wait: bool) -> Result<()> {
    ui::plan(6 + u32::from(wait));
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
    let facts = preflight::adb_side(&mut report, found_phone);
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
    preflight::fastboot_side(&mut bootloader, &serial, slot);
    bootloader.conclude("nura-install")?;

    ui::step(&steps::RESTART_TO_ANDROID);
    let mut report = Report::new();
    let found_phone = preflight::find_phone(&mut report);
    let again = preflight::adb_side(&mut report, found_phone);
    preflight::same_phone(&mut report, &facts, &again);
    report.conclude("nura-install")?;
    ui::section("Finish");
    ui::step(&steps::SUMMARY);
    ui::say("Nothing was written to the phone.");
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
