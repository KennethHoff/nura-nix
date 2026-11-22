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
    }
}

const ON_ANDROID: &str = "stays on Android, unchanged";
const NOT_TOUCHED: &str = "untouched";

pub const CHECK_COMPUTER: Step = tool(
    "Check this computer",
    Some("under 1 min"),
    &["- programs this run needs start", "- free disk space"],
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
        "- reports every missing or wrong answer, then stops",
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
        "- unpacks the images",
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
pub const SUMMARY: Step = tool(
    "Summarize",
    Some("under 10 s"),
    &["- says what was done and what is left"],
    Some("unchanged"),
);

#[cfg(test)]
pub const ALL: [&Step; 12] = [
    &FIND_BUILDS,
    &CHOOSE_UI,
    &CHECK_COMPUTER,
    &CONNECT,
    &CHECK_BOOTLOADER,
    &RESTART_TO_ANDROID,
    &CHECK_ADB,
    &FIND_IMAGES,
    &CONFIRM,
    &BACKUP,
    &TO_FASTBOOT,
    &SUMMARY,
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
    format!("== Step {n} of {total} · {actor}: {}", step.title)
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_block_aligns_its_labels_and_hangs_continuations() {
        assert_eq!(
            block(&CHECK_COMPUTER),
            [
                "  Takes:      under 1 min",
                "  Does:       - programs this run needs start",
                "              - free disk space",
                "  Phone:      untouched",
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
    fn a_header_numbers_the_step_and_names_its_actor() {
        assert_eq!(
            header(1, 14, &CHECK_COMPUTER),
            "== Step 1 of 14 · TOOL: Check this computer"
        );
        assert_eq!(
            header(3, 14, &TO_FASTBOOT),
            "== Step 3 of 14 · YOU: Put the phone in fastboot"
        );
    }
}
