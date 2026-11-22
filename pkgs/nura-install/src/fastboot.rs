//! fastboot: reading the phone's answers and the checks every group of writes depends on.
use crate::cmd::capture;
use crate::config;
use crate::fail::Result;
use crate::host::checked;
use crate::log;
use crate::phone::{Serial, Slot};
use crate::render::limit;
use crate::ui::{self, Job};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

const PRODUCT: &str = "FP6";

pub fn fastboot_cmd() -> Command {
    Command::new(config::fastboot_bin())
}

/// The value of `name` in fastboot's answer, which must contain it exactly once.
fn value_in(text: &str, name: &str) -> Result<String> {
    let prefix = format!("{name}:");
    let mut found = text.lines().filter_map(|l| {
        let l = l.strip_prefix("(bootloader) ").unwrap_or(l);
        l.strip_prefix(&prefix).map(|v| v.trim().to_string())
    });
    match (found.next(), found.next()) {
        (Some(v), None) if !v.is_empty() => Ok(v),
        (Some(_), None) | (None, _) => {
            Err(format!("the phone gave no value for `getvar {name}`; nothing was written").into())
        }
        (Some(_), Some(_)) => Err(format!(
            "the phone answered `getvar {name}` more than once; nothing was written"
        )
        .into()),
    }
}

/// Everything fastboot printed for `getvar name`, also in the transcript. A bootloader that does not
/// know a variable makes fastboot print `FAILED (remote: 'GetVar Variable Not found')` and exit 0
/// (AOSP fastboot.cpp, `DisplayVarOrError`), so the text decides, not the exit status.
fn getvar_text(serial: &Serial, name: &str) -> Result<String> {
    let out = capture(fastboot_cmd().args(["-s", serial.as_str(), "getvar", name]))?
        .success()
        .map_err(|e| format!("`getvar {name}` failed: {e}; nothing was written"))?;
    let text = format!("{}\n{}", out.stdout, out.stderr);
    log::line(&format!(
        "fastboot getvar {name}: {}",
        text.trim().replace('\n', " | ")
    ));
    Ok(text)
}

/// True when fastboot's text says the bootloader has no such variable (and gave no value).
fn says_not_found(text: &str, name: &str) -> bool {
    value_in(text, name).is_err()
        && text
            .lines()
            .any(|l| l.contains("FAILED") && l.to_lowercase().contains("variable not found"))
}

/// What `fastboot getvar all` listed: `(bootloader) name: value` lines (the FP6 prints some as
/// `name:value`, e.g. `has-slot:modem:yes`).
pub struct Vars(Vec<(String, String)>);

impl Vars {
    pub fn parse(text: &str) -> Vars {
        Vars(
            text.lines()
                .filter_map(|l| l.trim_end().strip_prefix("(bootloader) "))
                .filter_map(|l| {
                    l.split_once(": ")
                        .or_else(|| l.rsplit_once(':'))
                        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
                })
                .collect(),
        )
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n == name)
    }

    /// The same exactly-once rule as for a single `getvar`.
    fn once(&self, name: &str) -> Result<String> {
        let text: String = self
            .0
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(n, v)| format!("{n}: {v}\n"))
            .collect();
        value_in(&text, name)
    }
}

/// `getvar all`, once, with the raw answer in the transcript.
pub fn read_all(serial: &Serial) -> Result<Vars> {
    one_serial(serial)?;
    let text = getvar_text(serial, "all")?;
    let vars = Vars::parse(&text);
    if vars.len() == 0 {
        return Err("the phone's `getvar all` listed no variables; nothing was written".into());
    }
    Ok(vars)
}

fn parse_devices(text: &str) -> Result<Vec<Serial>> {
    let mut serials = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(serial), Some(state)) = (fields.next(), fields.next()) else {
            continue;
        };
        if line.contains("no permissions") || serial.starts_with('?') {
            return Err("fastboot sees a device it has no permission to use; replug the phone or check that this session is the active seat".into());
        }
        if state != "fastboot" {
            return Err(format!("device {serial} is in state {state:?}, not fastboot").into());
        }
        serials.push(Serial::new(serial)?);
    }
    Ok(serials)
}

pub fn serials() -> Result<Vec<Serial>> {
    let out = capture(fastboot_cmd().arg("devices"))?
        .success()
        .map_err(|e| format!("`fastboot devices` failed, so the phone's state is unknown: {e}"))?;
    parse_devices(&out.stdout)
}

/// What the wait says before it starts.
pub fn wait_naming() -> Result<String> {
    Ok(format!(
        "waiting for the phone in fastboot mode{}",
        crate::render::up_to(config::wait_limit()?)
    ))
}

/// Waits for the phone as a step of its own; the owner is putting it into fastboot meanwhile.
pub fn wait_for_phone() -> Result<()> {
    ui::run(
        Job::new("phone in fastboot mode").naming(wait_naming()?),
        wait_for_serial,
    )
    .map(drop)
}

/// The serial of the one phone in fastboot mode, polling until it shows up.
pub fn wait_for_serial() -> Result<(Serial, String)> {
    let started = Instant::now();
    let wait = config::wait_limit()?.unwrap_or(Duration::MAX);
    loop {
        match serials()?.as_slice() {
            [only] => return Ok((only.clone(), only.to_string())),
            [] if started.elapsed() < wait => sleep(Duration::from_secs(2)),
            [] => {
                return Err(format!(
                    "waited {} for the phone in fastboot mode and it did not show up; nothing was written to the phone. \
                     Next: hold Volume Down + Power until the fastboot screen shows, then run `nura-install` again",
                    limit(wait.as_secs())
                )
                .into())
            }
            many => {
                return Err(format!(
                    "{} fastboot devices are connected; connect only the phone",
                    many.len()
                )
                .into())
            }
        }
    }
}

fn one_serial(expected: &Serial) -> Result<()> {
    match serials()?.as_slice() {
        [only] if only == expected => Ok(()),
        [only] => Err(format!("the fastboot device is {only}, not {expected}").into()),
        [] => Err("no phone in fastboot mode".into()),
        many => Err(format!(
            "{} fastboot devices are connected; connect only the phone",
            many.len()
        )
        .into()),
    }
}

fn expect(vars: &Vars, name: &str, want: &str) -> Result<String> {
    let got = vars.once(name)?;
    if got == want {
        checked(name, &got);
        Ok(got)
    } else {
        Err(format!("the phone reports `{name}: {got}`, not {want}; nothing was written").into())
    }
}

fn expect_slot(vars: &Vars, recorded: Slot) -> Result<String> {
    let got = vars.once("current-slot")?;
    if got == recorded.as_str() {
        checked("current-slot", &got);
        return Ok(got);
    }
    Err(slot_changed(&got, recorded))
}

fn slot_changed(got: &str, recorded: Slot) -> crate::fail::Fail {
    format!(
        "fastboot says the current slot is `{got}`, but this phone was recorded on `{}`. The bootloader \
         switches slots on its own after repeated failed boots. Nothing was written. Do NOT switch slots \
         (no `fastboot set_active`, no `qbootctl`). See the Consequences section of \
         docs/adr/0001-guarded-fairphone-6-flashing.md before doing anything else.",
        recorded.as_str()
    )
    .into()
}

/// The slotted partitions a group writes; the bootloader publishes `partition-size:<name>_<slot>` for
/// them. (`has-slot:` is not usable: the FP6's ABL answers it for a few partitions only.)
pub const SLOTTED: [&str; 3] = ["boot", "vendor_boot", "dtbo"];
/// Partitions without a slot: sized under their plain name, absent under `_a`.
const UNSLOTTED: [&str; 1] = ["userdata"];

/// `0x...` with a value above zero.
fn parse_size(text: &str) -> Option<u64> {
    let digits = text.trim().strip_prefix("0x")?;
    u64::from_str_radix(digits, 16).ok().filter(|n| *n > 0)
}

fn sized(vars: &Vars, name: &str) -> Result<String> {
    let var = format!("partition-size:{name}");
    let got = vars.once(&var)?;
    let size = parse_size(&got).ok_or_else(|| {
        format!("the phone reports `{var}: {got}`, not a size above zero; nothing was written")
    })?;
    checked(&format!("partition-size:{name}"), &format!("{size:#x}"));
    Ok(format!("{size:#x}"))
}

fn unslotted_absent(vars: &Vars, name: &str) -> Result<String> {
    let var = format!("partition-size:{name}_a");
    let text = if vars.has(&var) {
        format!("{var}: {}\n", vars.once(&var)?)
    } else {
        format!("getvar:{var} FAILED (remote: 'GetVar Variable Not found')")
    };
    if says_not_found(&text, &var) {
        checked(&var, "absent");
        return Ok("absent".to_string());
    }
    match value_in(&text, &var) {
        Ok(v) => Err(format!(
            "the phone reports `{var}: {v}`, but {name} has no slot; nothing was written"
        )
        .into()),
        Err(_) => Err(format!(
            "the phone's answer to `getvar {var}` is neither a size nor \"not found\": {}; nothing was written",
            text.trim().replace('\n', " | ")
        )
        .into()),
    }
}

/// One check of the phone in fastboot: its label and the work that answers it.
pub struct Check<'a> {
    pub label: String,
    pub run: Box<dyn FnOnce() -> Result<String> + 'a>,
}

fn check<'a>(label: &str, run: impl FnOnce() -> Result<String> + 'a) -> Check<'a> {
    Check {
        label: format!("fastboot {label}"),
        run: Box::new(run),
    }
}

/// Every fastboot check, in the order they run, answered from the one `getvar all`.
pub fn checks<'a>(serial: &'a Serial, slot: Slot, vars: &'a Vars) -> Vec<Check<'a>> {
    let mut all = vec![
        check("serial", move || {
            one_serial(serial)?;
            checked("serial", serial.as_str());
            Ok(serial.to_string())
        }),
        check("product", move || expect(vars, "product", PRODUCT)),
        check("is-userspace", move || expect(vars, "is-userspace", "no")),
        check("unlocked", move || expect(vars, "unlocked", "yes")),
        check("current-slot", move || expect_slot(vars, slot)),
        check("slot-count", move || expect(vars, "slot-count", "2")),
        check("battery-soc-ok", move || {
            expect(vars, "battery-soc-ok", "yes")
        }),
    ];
    for partition in SLOTTED {
        for slot in ["a", "b"] {
            let name = format!("{partition}_{slot}");
            all.push(check(&format!("partition-size:{name}"), move || {
                sized(vars, &name)
            }));
        }
    }
    for partition in UNSLOTTED {
        all.push(check(&format!("partition-size:{partition}"), move || {
            sized(vars, partition)
        }));
        all.push(check(
            &format!("partition-size:{partition}_a (absent)"),
            move || unslotted_absent(vars, partition),
        ));
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_must_appear_exactly_once() {
        assert_eq!(
            value_in("product: FP6\nFinished.\n", "product").unwrap(),
            "FP6"
        );
        assert_eq!(
            value_in("(bootloader) product: FP6\n", "product").unwrap(),
            "FP6"
        );
        assert!(value_in("Finished.\n", "product").is_err());
        assert!(value_in("product:\n", "product").is_err());
        let both = value_in("unlocked: yes\nunlocked: no\n", "unlocked").unwrap_err();
        assert!(both.to_string().contains("unlocked"));
        assert!(value_in("unlocked: yes\nunlocked: yes\n", "unlocked").is_err());
        assert!(value_in("is-userspace: no\n", "unlocked").is_err());
    }

    #[test]
    fn sizes_are_nonzero_hex_and_an_unknown_variable_is_told_from_garbage() {
        assert_eq!(parse_size("0x6000000"), Some(0x600_0000));
        assert_eq!(parse_size("0x3425273000\n"), Some(0x34_2527_3000));
        assert_eq!(parse_size("0x0"), None);
        assert_eq!(parse_size("6000000"), None);
        assert_eq!(parse_size("0xzz"), None);
        let nf = "getvar:partition-size:userdata_a                  FAILED (remote: 'GetVar Variable Not found')\nFinished. Total time: 0.004s\n";
        assert!(says_not_found(nf, "partition-size:userdata_a"));
        let size = "(bootloader) partition-size:userdata: 0x3425273000\nFinished.\n";
        assert!(!says_not_found(size, "partition-size:userdata"));
        assert!(!says_not_found(
            "FAILED (remote: 'bad')\n",
            "partition-size:x"
        ));
    }

    #[test]
    fn getvar_all_is_parsed_from_bootloader_lines_in_both_shapes() {
        let all = "(bootloader) partition-size:vendor_boot_b: 0x6000000\n(bootloader) has-slot:modem:yes\n(bootloader) has-slot:boot:yes\n(bootloader) product: FP6\nall: \nFinished. Total time: 0.1s\n";
        let v = Vars::parse(all);
        assert_eq!(v.len(), 4);
        assert_eq!(v.once("partition-size:vendor_boot_b").unwrap(), "0x6000000");
        assert_eq!(v.once("has-slot:modem").unwrap(), "yes");
        assert_eq!(v.once("product").unwrap(), "FP6");
        assert!(v.once("unlocked").is_err());
        let twice = Vars::parse("(bootloader) unlocked: yes\n(bootloader) unlocked: no\n");
        assert!(twice
            .once("unlocked")
            .unwrap_err()
            .to_string()
            .contains("more than once"));
        assert!(v.has("product") && !v.has("unlocked"));
    }

    #[test]
    fn device_listing_refuses_what_is_not_a_fastboot_phone() {
        assert_eq!(parse_devices("ABC\tfastboot\n").unwrap().len(), 1);
        assert!(parse_devices("").unwrap().is_empty());
        assert_eq!(
            parse_devices("ABC\tfastboot\nDEF\tfastboot\n")
                .unwrap()
                .len(),
            2
        );
        assert!(parse_devices("ABC\tadb\n").is_err());
        assert!(parse_devices("????????????\tno permissions (user)\n").is_err());
        assert!(parse_devices("..\tfastboot\n").is_err());
    }
}
