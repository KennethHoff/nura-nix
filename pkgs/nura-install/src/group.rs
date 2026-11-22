//! The only path to the phone's storage. A `Group` is a batch of writes that must finish together; it
//! can only be built by `install` and `reboot`, each of which runs the
//! allowlist guard, and `execute` is the only thing that runs one.
use crate::cmd::{self, Sink, Spec};
use crate::config;
use crate::fail::Result;
use crate::fastboot::{self, fastboot_cmd};
use crate::host::checked;
use crate::images::{NuraImages, Verified};
use crate::log;
use crate::phone::{Serial, Slot};
use crate::prompt;
use crate::render::{human, limit};
use crate::signals::SignalBlock;
use crate::state::{self, Record, State};
use crate::steps::Step;
use crate::ui::{self, Job};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// The only writes this tool may ever run: (verb, partition).
const ALLOWED: [(&str, &str); 6] = [
    ("erase", "dtbo"),
    ("erase", "vendor_boot"),
    ("erase", "userdata"),
    ("flash", "userdata"),
    ("flash", "boot"),
    ("reboot", ""),
];

pub struct Phone {
    pub serial: Serial,
    pub cache: PathBuf,
    pub record: Record,
}

enum After {
    Set(State),
    Keep,
}

pub struct Group {
    title: &'static str,
    phrase: &'static str,
    writes: Vec<Vec<OsString>>,
    images: Vec<Verified>,
    /// After the writes only the phone's identity can be asked again: erasing changes other answers.
    identity_only: bool,
    in_progress: Option<State>,
    after: After,
    unchanged: &'static str,
    failed: &'static str,
}

fn words(ws: &[&OsStr]) -> Vec<OsString> {
    ws.iter().map(|w| w.to_os_string()).collect()
}

fn flash(partition: &str, image: &Verified) -> Vec<OsString> {
    words(&[
        "flash".as_ref(),
        partition.as_ref(),
        image.path().as_os_str(),
    ])
}

fn erase(partition: &str) -> Vec<OsString> {
    words(&["erase".as_ref(), partition.as_ref()])
}

fn display_args(args: &[OsString]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn guard(writes: &[Vec<OsString>]) -> Result<()> {
    for args in writes {
        let text = |i: usize| args.get(i).and_then(|a| a.to_str()).unwrap_or("");
        let (verb, partition) = (text(0), text(1));
        let shape = match verb {
            "reboot" => args.len() == 1,
            "erase" => args.len() == 2,
            "flash" => args.len() == 3,
            _ => false,
        };
        if !shape || !ALLOWED.contains(&(verb, partition)) {
            return Err(format!("refusing to run `fastboot {}`", display_args(args)).into());
        }
    }
    Ok(())
}

impl Group {
    fn checked(self) -> Result<Self> {
        guard(&self.writes)?;
        Ok(self)
    }

    pub fn install(nura: &NuraImages) -> Result<Self> {
        Group {
            title: "Install Nura",
            phrase: "install nura",
            writes: vec![
                erase("dtbo"),
                erase("vendor_boot"),
                flash("userdata", &nura.rootfs),
                flash("boot", &nura.boot),
            ],
            images: vec![nura.boot.clone(), nura.rootfs.clone()],
            identity_only: false,
            in_progress: Some(State::NuraInProgress),
            after: After::Set(State::NuraInstalled),
            unchanged: "The phone is unchanged Android and there is no record; boot it normally to run again (a new run makes a new backup).",
            failed: "the phone is in fastboot but may not boot",
        }
        .checked()
    }

    /// The reboot after the writes.
    pub fn reboot() -> Result<Self> {
        Group {
            title: "The reboot",
            phrase: "reboot",
            writes: vec![words(&["reboot".as_ref()])],
            images: Vec::new(),
            identity_only: true,
            in_progress: None,
            after: After::Keep,
            unchanged: "All writes are done and the record says `nura-installed`; the phone is still in fastboot. Reboot it with `fastboot -s {serial} reboot`.",
            failed: "the writes are done; reboot the phone by holding Power",
        }
        .checked()
    }
}

fn timeout_of(args: &[OsString]) -> Result<std::time::Duration> {
    let slow =
        args.first().is_some_and(|v| v == "flash") && args.get(1).is_some_and(|p| p == "userdata");
    config::write_timeout(slow)
}

/// A line in which fastboot says a command failed. Its exit status is not enough: it can exit 0 after
/// a remote `FAILED` (it does for `getvar`).
fn failed_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| l.contains("FAILED"))
}

/// Every image a group writes must fit the partition it goes to, by the bootloader's own sizes.
pub fn images_fit(serial: &Serial, slot: Slot, group: &Group) -> Result<()> {
    for args in &group.writes {
        if args.first().is_none_or(|v| v != "flash") {
            continue;
        }
        let (Some(partition), Some(path)) = (args.get(1), args.get(2)) else {
            continue;
        };
        let partition = partition.to_string_lossy();
        let target = if fastboot::SLOTTED.contains(&partition.as_ref()) {
            format!("{partition}_{}", slot.as_str())
        } else {
            partition.to_string()
        };
        let size = fastboot::partition_size(serial, &target)?;
        let image = std::fs::metadata(path)
            .map_err(|e| format!("reading {}: {e}", std::path::Path::new(path).display()))?
            .len();
        if image > size {
            return Err(format!(
                "the image for {partition} is {image} bytes but the phone's {target} holds {size}; nothing was written"
            )
            .into());
        }
        checked("image fits", &format!("{target}: {image} of {size}"));
    }
    Ok(())
}

fn approval_label(phrase: &str) -> &'static str {
    match phrase {
        "install nura" => "Install Nura (erases Android and all its data)",
        _ => "Reboot the phone (all writes are done)",
    }
}

fn gate(phone: &Phone, group: &Group) -> Result<bool> {
    ui::say(&format!("\n{} runs these writes:", group.title));
    for args in &group.writes {
        ui::say(&format!(
            "  fastboot -s {} {}",
            phone.serial,
            display_args(args)
        ));
    }
    prompt::approve(group.phrase, approval_label(group.phrase))
}

fn run_writes(phone: &Phone, group: &Group) -> Result<(Vec<String>, Option<String>)> {
    let mut ran = Vec::new();
    for args in &group.writes {
        let shown = display_args(args);
        let timeout = timeout_of(args)?;
        let label = args
            .iter()
            .take(2)
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let written = args
            .get(2)
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        let result = ui::run(
            Job::new(&label).naming(format!(
                "running `fastboot -s {} {shown}` (up to {})",
                phone.serial,
                limit(timeout.as_secs())
            )),
            || {
                let mut cmd = fastboot_cmd();
                cmd.args(["-s", phone.serial.as_str()]).args(args);
                let done = cmd::run(
                    &mut cmd,
                    Spec {
                        stdout: Sink::Relay,
                        stderr: Sink::Relay,
                        timeout: Some(timeout),
                        ..Spec::default()
                    },
                )?
                .success()?;
                if let Some(line) = failed_line(&format!("{}\n{}", done.stdout, done.stderr)) {
                    return Err(format!("fastboot exited 0 but printed `{line}`").into());
                }
                Ok((
                    (),
                    written.map_or(String::new(), |n| format!("{} written", human(n))),
                ))
            },
        );
        match result {
            Ok(()) => ran.push(shown),
            Err(e) => return Ok((ran, Some(format!("`fastboot {shown}`: {e}")))),
        }
    }
    Ok((ran, None))
}

/// Guard, gate, re-check (full, or identity only for the reboot), re-hash, in-progress record, the writes with signals held, final record.
pub fn execute(phone: &Phone, group: Group, applying: &'static Step) -> Result<()> {
    guard(&group.writes)?;
    if !gate(phone, &group)? {
        return Err(format!(
            "stopped at the `{}` gate. Nothing from this group was written. {}",
            group.phrase,
            group.unchanged.replace("{serial}", phone.serial.as_str())
        )
        .into());
    }
    ui::step(applying);
    if group.identity_only {
        fastboot::check_identity(&phone.serial, phone.record.slot)?;
    } else {
        fastboot::check_phone(&phone.serial, phone.record.slot)?;
    }
    images_fit(&phone.serial, phone.record.slot, &group)?;
    for image in &group.images {
        image.rehash()?;
        checked("image hash", &image.path().display().to_string());
    }
    if let Some(state) = group.in_progress {
        state::write(&phone.cache, &phone.serial, &phone.record.with_state(state))?;
    }
    let block = SignalBlock::new()?;
    ui::note_written();
    let (ran, failure) = run_writes(phone, &group)?;
    if let Some(failure) = failure {
        let record = group.in_progress.map_or(String::new(), |s| {
            format!(" The record says `{}`.", s.name())
        });
        return Err(format!(
            "{failure}. Writes that ran: {}.{record} {}.",
            if ran.is_empty() {
                "none".to_string()
            } else {
                ran.join("; ")
            },
            group.failed
        )
        .into());
    }
    match group.after {
        After::Set(state) => {
            state::write(&phone.cache, &phone.serial, &phone.record.with_state(state))?
        }
        After::Keep => {}
    }
    log::line(&format!("group `{}` finished", group.phrase));
    if block.take_pending()? {
        return Err(
            "interrupted while writing; the group finished and the record is current.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<OsString> {
        a.iter().map(OsString::from).collect()
    }

    fn bad(a: &[&str]) -> bool {
        Group {
            title: "t",
            phrase: "p",
            writes: vec![args(a)],
            images: Vec::new(),
            identity_only: false,
            in_progress: None,
            after: After::Keep,
            unchanged: "",
            failed: "",
        }
        .checked()
        .is_err()
    }

    #[test]
    fn a_group_with_a_forbidden_write_cannot_be_built() {
        assert!(bad(&["flash", "abl", "x"]));
        assert!(bad(&["flash", "vbmeta", "x"]));
        assert!(bad(&["flash", "super", "x"]));
        assert!(bad(&["flash", "misc", "x"]));
        assert!(bad(&["flash", "boot_a", "x"]));
        assert!(bad(&["erase", "xbl"]));
        assert!(bad(&["erase", "super"]));
        assert!(bad(&["erase", "misc"]));
        assert!(bad(&["erase", "metadata"]));
        assert!(bad(&["flash", "vendor_boot", "x"]));
        assert!(bad(&["flash", "dtbo", "x"]));
        assert!(bad(&["--set-active=a"]));
        assert!(bad(&["set_active", "a"]));
        assert!(bad(&["flashing", "lock"]));
        assert!(bad(&["oem", "lock"]));
        assert!(bad(&["reboot", "bootloader"]));
        assert!(bad(&[]));
    }

    #[test]
    fn a_failed_line_is_found_whatever_the_exit_status() {
        assert_eq!(
            failed_line("Erasing 'x'   FAILED (remote: 'boom')\nfinished\n"),
            Some("Erasing 'x'   FAILED (remote: 'boom')")
        );
        assert_eq!(failed_line("Sending 'boot' OKAY [0.1s]\nFinished.\n"), None);
    }

    #[test]
    fn writes_must_have_the_argument_count_of_their_verb() {
        assert!(bad(&["flash", "boot"]));
        assert!(bad(&["flash", "boot", "x", "y"]));
        assert!(bad(&["erase", "boot", "extra"]));
        assert!(bad(&["erase", "userdata", "extra"]));
        assert!(bad(&["erase"]));
        assert!(bad(&["reboot", ""]));
        assert!(!bad(&["erase", "userdata"]));
        assert!(!bad(&["flash", "boot", "x"]));
        assert!(!bad(&["reboot"]));
    }

    #[test]
    fn userdata_is_the_slow_write() {
        assert!(
            timeout_of(&args(&["flash", "userdata", "x"])).unwrap()
                > timeout_of(&args(&["flash", "boot", "x"])).unwrap()
        );
        assert_eq!(
            timeout_of(&args(&["erase", "userdata"])).unwrap(),
            timeout_of(&args(&["reboot"])).unwrap()
        );
    }

    #[test]
    fn the_reboot_group_passes_its_own_guard() {
        assert!(Group::reboot().is_ok());
    }
}
