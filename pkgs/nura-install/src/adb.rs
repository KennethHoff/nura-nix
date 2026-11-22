//! The Android phase: read-only questions over adb.
use crate::cmd::{capture, stdout_of};
use crate::config;
use crate::fail::Result;
use crate::host::checked;
use crate::phone::{Serial, Slot};
use std::process::Command;

pub const MIN_BATTERY: u32 = 50;

/// An entry of `adb devices`.
pub struct Device {
    pub serial: String,
    pub state: String,
}

fn adb_cmd() -> Command {
    Command::new(config::adb_bin())
}

pub fn adb_on(serial: &Serial) -> Command {
    let mut cmd = adb_cmd();
    cmd.args(["-s", serial.as_str()]);
    cmd
}

pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            Some(Device {
                serial: f.next()?.to_string(),
                state: f.next()?.to_string(),
            })
        })
        .collect()
}

pub fn devices() -> Result<Vec<Device>> {
    Ok(parse_devices(&stdout_of(adb_cmd().arg("devices"))?))
}

pub fn getprop(serial: &Serial, name: &str) -> Result<String> {
    let out = capture(adb_on(serial).args(["shell", "getprop", name]))?;
    let ok = out.ok();
    let value = out.stdout.trim();
    if !ok || value.is_empty() {
        return Err(format!("the phone gave no value for {name}").into());
    }
    Ok(value.to_string())
}

pub fn shell(serial: &Serial, command: &str) -> Result<String> {
    stdout_of(adb_on(serial).args(["shell", command]))
}

/// Sums the `Accounts: N` line of every user; anything unparsable is an error.
pub fn parse_accounts(text: &str) -> Result<u32> {
    let mut total = 0;
    let mut seen = false;
    for line in text.lines() {
        if let Some(n) = line.trim().strip_prefix("Accounts: ") {
            let n: u32 = n
                .trim()
                .parse()
                .map_err(|_| "dumpsys account has an `Accounts:` line without a count")?;
            total += n;
            seen = true;
        }
    }
    if seen {
        Ok(total)
    } else {
        Err("dumpsys account has no `Accounts:` line, so the sign-in state is unknown".into())
    }
}

pub fn parse_battery(text: &str) -> Result<u32> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("level: "))
        .and_then(|n| n.trim().parse().ok())
        .filter(|n| *n <= 100)
        .ok_or_else(|| "dumpsys battery has no readable level".into())
}

pub fn slot_of(serial: &Serial) -> Result<Slot> {
    let suffix = getprop(serial, "ro.boot.slot_suffix")?;
    let slot = suffix
        .strip_prefix('_')
        .and_then(Slot::parse)
        .ok_or_else(|| format!("unexpected slot suffix {suffix:?}"))?;
    checked("slot", slot.as_str());
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_summed_over_users_and_unparsable_stops() {
        assert_eq!(
            parse_accounts("User 0:\n  Accounts: 0\nUser 10:\n  Accounts: 0\n").unwrap(),
            0
        );
        assert_eq!(parse_accounts("  Accounts: 1\n  Accounts: 2").unwrap(), 3);
        assert_eq!(
            parse_accounts("  Accounts: 2\r\n  Accounts: 1\r\n").unwrap(),
            3
        );
        assert!(parse_accounts("no such line").is_err());
        assert!(parse_accounts("  Accounts: many").is_err());
    }

    #[test]
    fn battery_level_must_be_readable() {
        assert_eq!(
            parse_battery("Current Battery Service state:\n  level: 73\n").unwrap(),
            73
        );
        assert_eq!(
            parse_battery("Current Battery Service state:\r\n  level: 73\r\n").unwrap(),
            73
        );
        assert!(parse_battery("  level: x").is_err());
        assert!(parse_battery("  level: 900").is_err());
        assert!(parse_battery("").is_err());
    }
}
