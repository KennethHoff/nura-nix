//! Which run this is, decided before anything is printed from what the computer can see: a phone in
//! fastboot, a phone over adb, the records. It only reads; the Preflight then checks everything.
use crate::adb;
use crate::fail::{Fail, Result};
use crate::fastboot;
use crate::phone::Serial;
use crate::state::{self, Record};
use std::path::Path;

pub enum Route {
    /// Nura from Android over adb; `wait` when no phone is visible yet and the owner has to connect one.
    Fresh { wait: bool },
    /// Nura is installed or half installed and the phone is in fastboot: reinstall or finish the install.
    Nura { serial: Serial, record: Record },
}

fn nothing<T>(msg: &str) -> Result<T> {
    Err(Fail::NothingToDo(msg.to_string()))
}

/// What a recorded phone gets: in fastboot, Nura again; otherwise there is nothing to do but say how.
fn by_state(serial: Serial, record: Record, in_fastboot: bool) -> Result<Route> {
    if in_fastboot {
        return Ok(Route::Nura { serial, record });
    }
    nothing(&format!(
        "phone {serial} is `{}`. To install Nura again, power it off, hold Volume Down + Power and run `nura-install` from fastboot.",
        record.state.name()
    ))
}

pub fn decide(cache: &Path) -> Result<Route> {
    let in_fastboot = fastboot::serials()?;
    match in_fastboot.as_slice() {
        [] => {}
        [serial] => {
            let Some(record) = state::read(cache, serial)? else {
                return nothing(&format!(
                    "the phone {serial} in fastboot has no record, so it was not put there by this tool. \
                     A fresh install starts from Android over adb: boot the phone normally and run `nura-install`."
                ));
            };
            return by_state(serial.clone(), record, true);
        }
        many => {
            return Err(format!(
                "{} fastboot devices are connected; connect only the phone",
                many.len()
            )
            .into())
        }
    }
    if !adb::devices()?.is_empty() {
        return Ok(Route::Fresh { wait: false });
    }
    let mut records = state::all(cache)?;
    match records.len() {
        0 => Ok(Route::Fresh { wait: true }),
        1 => {
            let (serial, record) = records.remove(0);
            by_state(serial, record, false)
        }
        n => Err(format!(
            "{n} phones have a record and none is in fastboot; put the one you mean in fastboot (Volume Down + Power) and run again"
        )
        .into()),
    }
}
