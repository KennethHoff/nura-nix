mod cmd;
mod config;
mod fail;
mod host;
mod log;
mod preflight;
mod render;
mod signals;
mod steps;
mod ui;

use fail::{Fail, Result};
use preflight::Report;
use std::process::ExitCode;

const USAGE: &str = "Usage: nura-install

Takes no arguments: it checks that this computer has what a Nura install needs. Nothing is
written to a phone.

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

/// Ctrl-C, hang-up or terminate: ends the run at once.
pub fn interrupted() -> ! {
    ui::stop_live();
    ui::kill_child();
    ui::end_step();
    ui::say("\ninterrupted");
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
    ui::plan(1);
    ui::section("Preflight");
    ui::step(&steps::CHECK_COMPUTER);
    let mut report = Report::new();
    preflight::computer(&mut report, &cache);
    report.conclude("nura-install")
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
