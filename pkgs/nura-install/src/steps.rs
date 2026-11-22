//! The text under every step header, in one table so it can be reviewed on one screen. A step has
//! exactly one actor: the owner (`YOU`, a `Do:` line) or this tool (`TOOL`, a `Does:` line), and the
//! steps run in strict order. `Takes:` is a static typical range, never a live estimate. `Do:` lines are `[ ]` checklist items,
//! `Does:` lines are `-` bullets.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    You,
    Tool,
}

pub struct Step {
    pub actor: Actor,
    pub title: &'static str,
    /// What the terminal title says while the owner has to act, in a few words.
    pub short: &'static str,
    pub takes: Option<&'static str>,
    pub text: &'static [&'static str],
    pub phone: Option<&'static str>,
    pub after: &'static [&'static str],
    pub no_return: bool,
}

const fn you(
    title: &'static str,
    short: &'static str,
    takes: Option<&'static str>,
    text: &'static [&'static str],
    phone: Option<&'static str>,
) -> Step {
    Step {
        actor: Actor::You,
        title,
        short,
        takes,
        text,
        phone,
        after: &[],
        no_return: false,
    }
}

const fn tool(
    title: &'static str,
    takes: Option<&'static str>,
    text: &'static [&'static str],
    phone: Option<&'static str>,
) -> Step {
    Step {
        actor: Actor::Tool,
        title,
        short: "",
        takes,
        text,
        phone,
        after: &[],
        no_return: false,
    }
}

const fn approval(
    title: &'static str,
    short: &'static str,
    text: &'static [&'static str],
    after: &'static [&'static str],
) -> Step {
    Step {
        actor: Actor::You,
        title,
        short,
        takes: None,
        text,
        phone: None,
        after,
        no_return: true,
    }
}

const ON_ANDROID: &str = "stays on Android, unchanged";
const HANDS_OFF: &str = "don't touch it or the cable";
const NOT_TOUCHED: &str = "untouched";

pub const CHECK_COMPUTER: Step = tool(
    "Check this computer",
    Some("under 1 min"),
    &["- programs this run needs start", "- free disk space"],
    Some(NOT_TOUCHED),
);
pub const CHECK_RECORD: Step = tool(
    "Check this computer and the record",
    Some("under 1 min"),
    &[
        "- programs this run needs start",
        "- free disk space",
        "- reads the phone's record",
    ],
    Some(NOT_TOUCHED),
);
pub const CONNECT: Step = you(
    "Connect the phone with USB debugging",
    "connect the phone",
    Some("about 1 min"),
    &[
        "[ ] Boot into Android",
        "[ ] Settings > About phone: tap Build number 7 times",
        "[ ] Settings > System > Developer options: USB debugging on",
        "[ ] Connect the cable straight to this computer (no hub: a dropped link mid-write is the main risk)",
        "[ ] Accept the RSA prompt (lets this computer use adb)",
    ],
    Some(ON_ANDROID),
);
pub const CHECK_BOOTLOADER: Step = tool(
    "Check the bootloader",
    Some("under 10 s"),
    &[
        "- reads `getvar all` once",
        "- serial, product, unlocked, slot, update status, battery",
        "- partition sizes",
        "- `unlocked: no` is not a failure here: the run then unlocks the bootloader first",
        "- reports every other missing or wrong answer, then stops",
    ],
    Some("unchanged"),
);
pub const CHOOSE_UI: Step = you(
    "Choose Nura's interface",
    "choose an interface",
    Some("under 1 min"),
    &["[ ] Pick an interface with the arrow keys and Enter (Esc stops the run)"],
    Some(NOT_TOUCHED),
);
pub const FIND_BUILDS: Step = tool(
    "Find Nura's builds for the phone",
    Some("under 1 min"),
    &[
        "- downloads Nura's image index (a small file, fetched fresh)",
        "- lists the interfaces it builds for this phone and the newest build of each",
    ],
    Some("nothing sent about the phone"),
);
pub const RESTART_TO_ANDROID: Step = you(
    "Restart into Android",
    "restart into Android",
    Some("1-3 min"),
    &[
        "[ ] Hold Power until the phone restarts",
        "[ ] Boot into Android",
        "[ ] Keep the cable connected",
    ],
    Some("leaves fastboot, nothing written"),
);
pub const CHECK_ADB: Step = tool(
    "Check the phone over adb",
    Some("under 1 min"),
    &["- reads serial, slot, accounts, battery (no account may be signed in; battery at least 50 %)"],
    Some(ON_ANDROID),
);
pub const FIND_IMAGES: Step = tool(
    "Find, download and verify the images",
    Some("1-20 min (cached files are only re-checked)"),
    &[
        "- downloads the newest build of the chosen interface, checks it against the hashes in Nura's index",
        "- unpacks the images and hashes them",
    ],
    Some("nothing sent"),
);
pub const FIND_NURA: Step = tool(
    "Download and verify the Nura images",
    Some("1-15 min (cached files are only re-checked)"),
    &[
        "- downloads missing files",
        "- checks every file against its hash in Nura's index",
        "- unpacks the images and hashes them",
    ],
    Some("nothing sent"),
);
pub const CONFIRM: Step = you(
    "Confirm what is lost",
    "confirm",
    Some("about 1 min"),
    &[
        "[ ] Settings > Security > Screen lock: None (also turns off Factory Reset Protection)",
        "[ ] Then choose below: back up the phone, or skip the backup (that takes a second choice, with the cursor on Stop)",
    ],
    Some(ON_ANDROID),
);
pub const BACKUP: Step = tool(
    "Back up the phone",
    Some("5-20 min (by stored data)"),
    &[
        "- copies /sdcard",
        "- exports contacts, SMS, call log, calendar",
        "- checks the copy against hashes from the phone (a broken copy is caught before the wipe)",
        "- skipped when you chose so in the previous step",
    ],
    Some(ON_ANDROID),
);
pub const TO_FASTBOOT: Step = you(
    "Put the phone in fastboot",
    "Volume Down + Power",
    Some("about 1 min"),
    &[
        "[ ] Hold Power, tap Power off",
        "[ ] Hold Volume Down + Power until the fastboot screen shows (`adb reboot bootloader` would write to the phone)",
        "[ ] Keep the cable connected",
    ],
    Some("nothing written yet"),
);
pub const ENABLE_OEM: Step = you(
    "Turn on OEM unlocking",
    "turn on OEM unlocking",
    Some("about 1 min"),
    &[
        "[ ] Settings > System > Developer options: OEM unlocking on (the phone needs internet; it may ask for your PIN)",
        "[ ] Keep USB debugging on and the cable connected",
    ],
    Some(ON_ANDROID),
);
pub const TO_FASTBOOT_WIPED: Step = you(
    "Put the phone in fastboot",
    "Volume Down + Power",
    Some("1-3 min"),
    &[
        "[ ] Let the phone restart on its own; it was wiped, so don't set it up",
        "[ ] Hold Power, tap Power off",
        "[ ] Hold Volume Down + Power until the fastboot screen shows",
        "[ ] Keep the cable connected",
    ],
    Some("wiped by the unlock, nothing else written"),
);
pub const CHECK_LOCKED: Step = tool(
    "Check the bootloader is locked",
    Some("under 10 s"),
    &["- reads serial, slot, unlock state (it must say `unlocked: no`)"],
    Some("unchanged"),
);
pub const CHECK_UNLOCKED: Step = tool(
    "Check the bootloader is unlocked",
    Some("under 10 s"),
    &["- reads serial, slot, unlock state (it must say `unlocked: yes`)"],
    Some("unchanged"),
);
pub const APPROVE_UNLOCK: Step = approval(
    "Approve the unlock",
    "choose Unlock the bootloader",
    &[],
    &[
        "- erases the phone: Android and all its data (the backup is taken)",
        "- the phone asks for its own confirmation, then restarts wiped",
    ],
);
pub const UNLOCK: Step = tool(
    "Unlock the bootloader",
    Some("1-3 min"),
    &[
        "- re-checks the phone (same serial, still locked, same slot)",
        "- sends `fastboot flashing unlock`",
        "- waits for the confirmation on the phone",
    ],
    Some("asks you to confirm with its buttons, then wipes and restarts"),
);
pub const APPROVE_UNLOCK_CRITICAL: Step = approval(
    "Approve the critical unlock",
    "choose Unlock the critical partitions",
    &[],
    &[
        "- erases the phone a second time",
        "- the phone asks for its own confirmation, then restarts wiped",
    ],
);
pub const UNLOCK_CRITICAL: Step = tool(
    "Unlock the critical partitions",
    Some("1-3 min"),
    &[
        "- re-checks the phone (same serial, unlocked, same slot)",
        "- sends `fastboot flashing unlock_critical`",
        "- waits for the confirmation on the phone",
    ],
    Some("asks you to confirm with its buttons, then wipes and restarts"),
);
pub const CHECK_FASTBOOT: Step = tool(
    "Check the phone in fastboot",
    Some("under 10 s"),
    &["- reads serial, slot, partition layout, unlock state, update status, battery"],
    Some("unchanged"),
);
pub const CHECK_FASTBOOT_RECORDED: Step = tool(
    "Check the phone in fastboot",
    Some("under 10 s"),
    &["- reads serial, slot, partition layout, unlock state, update status, battery"],
    Some("unchanged"),
);
pub const APPROVE_INSTALL: Step = approval(
    "Approve the install",
    "choose Install Nura",
    &[],
    &["- erases Android and all its data"],
);
pub const INSTALL: Step = tool(
    "Install Nura",
    Some("3-10 min"),
    &[
        "- re-checks the phone (same serial, still unlocked, same slot)",
        "- erases dtbo, vendor_boot",
        "- writes userdata, boot",
    ],
    Some(HANDS_OFF),
);
pub const APPROVE_REINSTALL: Step = approval(
    "Approve the reinstall",
    "choose Reinstall Nura",
    &[],
    &["- erases Nura and all its data, installs it again"],
);
pub const REINSTALL: Step = tool(
    "Reinstall Nura",
    Some("3-10 min"),
    &[
        "- re-checks the phone (same serial, still unlocked, same slot)",
        "- erases dtbo, vendor_boot",
        "- writes userdata, boot again",
    ],
    Some(HANDS_OFF),
);
pub const APPROVE_REBOOT: Step = you(
    "Approve the reboot",
    "choose Reboot",
    None,
    &[],
    Some("in fastboot, all writes done; after `stop`, hold Power to restart it"),
);
pub const REBOOT: Step = tool(
    "Restart the phone",
    Some("under 10 s, then 1-3 min first start"),
    &["- sends `fastboot reboot`"],
    Some("restarts into the new system"),
);
pub const START_SSHD: Step = you(
    "Start sshd on Nura",
    "start sshd on Nura",
    Some("1-3 min for Nura to start, then about 1 min"),
    &[
        "[ ] Wait for Nura to start",
        "[ ] Log in: user / 147147",
        "[ ] Terminal app on the phone: `sudo systemctl start sshd` (lets this computer copy the partitions)",
        "[ ] Then choose below: copy, or skip the copy (Nura stays installed; `nura-install` later does only the copy)",
    ],
    Some("runs Nura, only sshd starts"),
);
pub const COPY: Step = tool(
    "Copy the per-phone partitions",
    Some("1-5 min"),
    &[
        "- reads persist, modemst1, modemst2, fsg, fsc over the USB network link (IMEI and calibration; no download replaces them)",
        "- checks each against the phone's hash",
        "- stops sshd (its password is public)",
    ],
    Some("read only, sshd stopped at the end"),
);
pub const SUMMARY: Step = tool(
    "Summarize",
    Some("under 10 s"),
    &["- says what was done and what is left"],
    Some("unchanged"),
);

#[cfg(test)]
pub const ALL: [&Step; 32] = [
    &FIND_BUILDS,
    &CHOOSE_UI,
    &CHECK_COMPUTER,
    &CHECK_RECORD,
    &CONNECT,
    &CHECK_BOOTLOADER,
    &RESTART_TO_ANDROID,
    &CHECK_ADB,
    &FIND_IMAGES,
    &FIND_NURA,
    &CONFIRM,
    &BACKUP,
    &TO_FASTBOOT,
    &CHECK_FASTBOOT,
    &CHECK_FASTBOOT_RECORDED,
    &APPROVE_INSTALL,
    &INSTALL,
    &APPROVE_REINSTALL,
    &REINSTALL,
    &APPROVE_REBOOT,
    &REBOOT,
    &START_SSHD,
    &COPY,
    &SUMMARY,
    &ENABLE_OEM,
    &TO_FASTBOOT_WIPED,
    &CHECK_LOCKED,
    &CHECK_UNLOCKED,
    &APPROVE_UNLOCK,
    &UNLOCK,
    &APPROVE_UNLOCK_CRITICAL,
    &UNLOCK_CRITICAL,
];

const LABEL: usize = 12;

fn field(out: &mut Vec<String>, label: &str, lines: &[&str]) {
    for (i, line) in lines.iter().enumerate() {
        let name = if i == 0 { label } else { "" };
        out.push(format!("  {name:<LABEL$}{line}"));
    }
}

pub fn header(n: u32, total: u32, step: &Step) -> String {
    let actor = match step.actor {
        Actor::You => "YOU",
        Actor::Tool => "TOOL",
    };
    if step.no_return {
        format!(
            "== Step {n} of {total} · {actor}: {:<30}  POINT OF NO RETURN",
            step.title
        )
    } else {
        format!("== Step {n} of {total} · {actor}: {}", step.title)
    }
}

/// The labelled lines under a header; a line appears only when it applies.
pub fn block(step: &Step) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(takes) = step.takes {
        field(&mut out, "Takes:", &[takes]);
    }
    let label = match step.actor {
        Actor::You => "Do:",
        Actor::Tool => "Does:",
    };
    field(&mut out, label, step.text);
    if let Some(phone) = step.phone {
        field(&mut out, "Phone:", &[phone]);
    }
    field(&mut out, "After this:", step.after);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_block_aligns_its_labels_and_hangs_continuations() {
        assert_eq!(
            block(&APPROVE_REINSTALL),
            ["  After this: - erases Nura and all its data, installs it again",]
        );
        assert_eq!(
            block(&INSTALL),
            [
                "  Takes:      3-10 min",
                "  Does:       - re-checks the phone (same serial, still unlocked, same slot)",
                "              - erases dtbo, vendor_boot",
                "              - writes userdata, boot",
                "  Phone:      don't touch it or the cable",
            ]
        );
    }

    #[test]
    fn a_step_has_one_actor_and_never_both_kinds_of_action_line() {
        for step in ALL {
            let lines = block(step);
            let do_lines = lines.iter().filter(|l| l.starts_with("  Do:")).count();
            let does_lines = lines.iter().filter(|l| l.starts_with("  Does:")).count();
            let has_text = usize::from(!step.text.is_empty());
            match step.actor {
                Actor::You => assert_eq!((do_lines, does_lines), (has_text, 0), "{}", step.title),
                Actor::Tool => assert_eq!((do_lines, does_lines), (0, 1), "{}", step.title),
            }
            assert!(step.actor == Actor::You || has_text == 1, "{}", step.title);
        }
    }

    #[test]
    fn every_point_of_no_return_is_a_you_step_with_the_marker_and_an_after_line() {
        for step in [
            &APPROVE_INSTALL,
            &APPROVE_REINSTALL,
            &APPROVE_UNLOCK,
            &APPROVE_UNLOCK_CRITICAL,
        ] {
            assert!(step.no_return && step.actor == Actor::You, "{}", step.title);
            assert!(header(8, 14, step).ends_with("POINT OF NO RETURN"));
            assert!(block(step).iter().any(|l| l.starts_with("  After this: ")));
        }
    }

    #[test]
    fn only_the_approvals_of_irreversible_writes_carry_the_marker() {
        let marked: Vec<&str> = ALL
            .iter()
            .filter(|s| s.no_return)
            .map(|s| s.title)
            .collect();
        assert_eq!(
            marked,
            [
                "Approve the install",
                "Approve the reinstall",
                "Approve the unlock",
                "Approve the critical unlock"
            ]
        );
        assert_eq!(
            header(1, 14, &CHECK_COMPUTER),
            "== Step 1 of 14 · TOOL: Check this computer"
        );
    }
}
