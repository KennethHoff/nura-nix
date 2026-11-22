//! The unlock section, run only when the bootloader says `unlocked: no`: OEM unlocking on the phone,
//! then `flashing unlock` and `flashing unlock_critical`, each behind its own approval, with the
//! record `unlock-in-progress` from before the first command. Fastboot reports no critical-unlock
//! variable, so completion is read from `unlocked` and from the commands' own success.
use crate::adb;
use crate::config;
use crate::fail::Result;
use crate::fastboot::{self, Lock};
use crate::group::{execute, Group, Origin, Phone};
use crate::prompt;
use crate::render::up_to;
use crate::steps;
use crate::ui::{self, Job};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How many steps the section adds to a run, for the step count.
pub const FROM_ANDROID: u32 = 9;
/// Resuming from fastboot while the phone is still locked, and while only the critical unlock is left.
pub const RESUME_LOCKED: u32 = 8;
pub const RESUME_UNLOCKED: u32 = 4;

/// Where the section starts: on Android (OEM unlocking, then fastboot), or in fastboot already.
#[derive(Clone, Copy)]
pub enum Start {
    Android,
    Fastboot { unlocked: bool },
}

const UNLOCK_NOTE: &str = "Unlocking wipes the phone, twice. The phone restarts into a wiped Android after each one: don't set it up, go straight back to fastboot.";

/// OEM unlocking, as far as the phone can say so over adb; when it cannot, the owner confirms.
fn oem_unlocking(serial: &crate::phone::Serial) -> Result<()> {
    let naming = format!(
        "waiting for OEM unlocking to be on{}",
        up_to(config::wait_limit()?)
    );
    let started = Instant::now();
    let wait = config::wait_limit()?.unwrap_or(Duration::MAX);
    let answer = ui::run(Job::new("OEM unlocking").naming(naming), || loop {
        let on = adb::oem_unlocking(serial)?;
        if on != Some(false) || started.elapsed() >= wait {
            let shown = match on {
                Some(true) => "on".to_string(),
                Some(false) => "off".to_string(),
                None => format!("`{}` not reported", adb::OEM_UNLOCK_PROP),
            };
            return Ok((on, shown));
        }
        sleep(Duration::from_secs(2));
    })?;
    match answer {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "the phone says OEM unlocking is off (`{}` is 0); nothing was written. Turn it on in Settings > System > Developer options and run `nura-install` again",
            adb::OEM_UNLOCK_PROP
        )
        .into()),
        None => {
            ui::say("  The phone does not report OEM unlocking, so it cannot be checked from here.");
            if prompt::approve("oem unlocking", "OEM unlocking is on (confirm)")? {
                Ok(())
            } else {
                Err("stopped; nothing was written to the phone".into())
            }
        }
    }
}

fn first(phone: &Phone, origin: Origin) -> Result<()> {
    ui::step(&steps::APPROVE_UNLOCK);
    ui::say(UNLOCK_NOTE);
    execute(phone, Group::unlock(origin)?, &steps::UNLOCK)
}

fn second(phone: &Phone) -> Result<()> {
    ui::step(&steps::APPROVE_UNLOCK_CRITICAL);
    execute(phone, Group::unlock_critical()?, &steps::UNLOCK_CRITICAL)
}

/// Runs the section up to and including the owner's way back into fastboot after the last wipe; the
/// caller waits for the phone there and goes on to the install.
pub fn run(phone: &Phone, start: Start, origin: Origin) -> Result<()> {
    ui::section("Unlock the bootloader");
    match start {
        Start::Android => {
            ui::step(&steps::ENABLE_OEM);
            oem_unlocking(&phone.serial)?;
            ui::step(&steps::TO_FASTBOOT);
            fastboot::wait_for_phone()?;
            ui::step(&steps::CHECK_LOCKED);
            fastboot::check_identity(&phone.serial, phone.record.slot, Lock::Locked)?;
            first(phone, origin)?;
        }
        Start::Fastboot { unlocked: false } => first(phone, origin)?,
        Start::Fastboot { unlocked: true } => {}
    }
    if !matches!(start, Start::Fastboot { unlocked: true }) {
        ui::step(&steps::TO_FASTBOOT_WIPED);
        fastboot::wait_for_phone()?;
        ui::step(&steps::CHECK_UNLOCKED);
        fastboot::check_identity(&phone.serial, phone.record.slot, Lock::Unlocked)?;
    }
    second(phone)?;
    ui::step(&steps::TO_FASTBOOT_WIPED);
    Ok(())
}
