//! The per-phone record: where the phone is, written atomically around each group of writes.
use crate::cmd::stdout_of;
use crate::fail::Result;
use crate::host::{is_sha256, mkdir, sync_path};
use crate::phone::{Serial, Slot};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NuraInProgress,
    NuraInstalled,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::NuraInProgress => "nura-in-progress",
            State::NuraInstalled => "nura-installed",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [State::NuraInProgress, State::NuraInstalled]
            .into_iter()
            .find(|state| state.name() == s)
    }
}

/// The Nura build and the hashes of the images that were written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NuraIdentity {
    pub build: String,
    pub boot_sha256: String,
    pub rootfs_sha256: String,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub state: State,
    pub slot: Slot,
    pub backup: String,
    pub nura: NuraIdentity,
}

impl Record {
    pub fn with_state(&self, state: State) -> Record {
        Record {
            state,
            ..self.clone()
        }
    }

    pub fn with_nura(&self, nura: NuraIdentity) -> Record {
        Record {
            nura,
            ..self.clone()
        }
    }
}

fn phone_dir(cache: &Path, serial: &Serial) -> PathBuf {
    cache.join("phones").join(serial.as_str())
}

fn file(cache: &Path, serial: &Serial) -> PathBuf {
    phone_dir(cache, serial).join("state.json")
}

const FIELDS: &str = r#"[.state, .slot, .backup, .nura_build, .nura_boot_sha256, .nura_rootfs_sha256] | map(if type == "string" then . else "" end) | @tsv"#;

pub fn read(cache: &Path, serial: &Serial) -> Result<Option<Record>> {
    let path = file(cache, serial);
    if !path.exists() {
        return Ok(None);
    }
    let corrupt = |why: String| format!("the record {} is {why}; fix or remove it", path.display());
    let text = stdout_of(Command::new("jq").args(["-r", FIELDS]).arg(&path))
        .map_err(|e| corrupt(format!("unreadable ({e})")))?;
    let fields: Vec<&str> = text.trim_end_matches('\n').split('\t').collect();
    let [state, slot, backup, nura_build, nura_boot, nura_rootfs] = fields[..] else {
        return Err(corrupt("malformed".into()).into());
    };
    let state =
        State::parse(state).ok_or_else(|| corrupt(format!("in an unknown state {state:?}")))?;
    let slot = Slot::parse(slot).ok_or_else(|| corrupt(format!("on an unknown slot {slot:?}")))?;
    for (name, value) in [("backup", backup), ("nura_build", nura_build)] {
        if value.is_empty() {
            return Err(corrupt(format!("missing `{name}`")).into());
        }
    }
    for (name, value) in [
        ("nura_boot_sha256", nura_boot),
        ("nura_rootfs_sha256", nura_rootfs),
    ] {
        if !is_sha256(value) {
            return Err(corrupt(format!("missing a valid `{name}`")).into());
        }
    }
    Ok(Some(Record {
        state,
        slot,
        backup: backup.to_string(),
        nura: NuraIdentity {
            build: nura_build.to_string(),
            boot_sha256: nura_boot.to_string(),
            rootfs_sha256: nura_rootfs.to_string(),
        },
    }))
}

/// Every phone with a record, for runs that start before the phone shows up anywhere.
pub fn all(cache: &Path) -> Result<Vec<(Serial, Record)>> {
    let dir = cache.join("phones");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("reading {}: {e}", dir.display()))?;
        let serial = Serial::new(&entry.file_name().to_string_lossy())?;
        if let Some(rec) = read(cache, &serial)? {
            found.push((serial, rec));
        }
    }
    found.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    Ok(found)
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render(rec: &Record) -> String {
    let fields = [
        ("state", rec.state.name()),
        ("slot", rec.slot.as_str()),
        ("backup", &rec.backup),
        ("nura_build", &rec.nura.build),
        ("nura_boot_sha256", &rec.nura.boot_sha256),
        ("nura_rootfs_sha256", &rec.nura.rootfs_sha256),
    ];
    let body: Vec<String> = fields
        .iter()
        .map(|(k, v)| format!("{}:{}", json_string(k), json_string(v)))
        .collect();
    format!("{{{}}}\n", body.join(","))
}

/// Temp file, fsync, rename, then fsync of the directory.
pub fn write(cache: &Path, serial: &Serial, rec: &Record) -> Result<()> {
    let dir = phone_dir(cache, serial);
    mkdir(&dir)?;
    let tmp = dir.join("state.json.tmp");
    let path = dir.join("state.json");
    let io = |what: &str, e: std::io::Error| format!("{what} {}: {e}", path.display());
    let mut f = File::create(&tmp).map_err(|e| io("creating the record for", e))?;
    f.write_all(render(rec).as_bytes())
        .and_then(|()| f.sync_all())
        .map_err(|e| io("writing the record for", e))?;
    fs::rename(&tmp, &path).map_err(|e| io("replacing", e))?;
    sync_path(&dir)
}

/// One line about the record, for the end of a stopped run.
pub fn describe(cache: &Path, serial: &Serial) -> String {
    match read(cache, serial) {
        Ok(None) => format!("There is no record for phone {serial}."),
        Ok(Some(rec)) => format!("The record for phone {serial} says `{}`.", rec.state.name()),
        Err(e) => format!("The record for phone {serial} cannot be used: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn record() -> Record {
        Record {
            state: State::NuraInstalled,
            slot: Slot::A,
            backup: "/x\"y".into(),
            nura: NuraIdentity {
                build: "n".into(),
                boot_sha256: "4".repeat(64),
                rootfs_sha256: "5".repeat(64),
            },
        }
    }

    fn cache_with(text: &str) -> (PathBuf, Serial) {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nura-state-{}-{n}", std::process::id()));
        let serial = Serial::new("S1").unwrap();
        fs::create_dir_all(phone_dir(&dir, &serial)).unwrap();
        fs::write(file(&dir, &serial), text).unwrap();
        (dir, serial)
    }

    #[test]
    fn record_round_trips_through_its_json() {
        let text = render(&record());
        let (cache, serial) = cache_with(&text);
        let back = read(&cache, &serial).unwrap().unwrap();
        assert_eq!(back.backup, "/x\"y");
        assert_eq!(back.nura, record().nura);
        assert_eq!(back.slot, Slot::A);
        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn a_corrupt_or_partial_record_is_an_error_never_no_record() {
        let good = render(&record());
        let broken = [
            "not json".to_string(),
            "{}".to_string(),
            good.replace("nura-installed", "other"),
            good.replace("\"slot\":\"a\"", "\"slot\":\"c\""),
            good.replace("\"backup\":\"/x\\\"y\"", "\"backup\":\"\""),
            good.replace("\"nura_build\":\"n\"", "\"nura_build\":3"),
            good.replace(&"5".repeat(64), "short"),
        ];
        for text in broken {
            let (cache, serial) = cache_with(&text);
            let err = read(&cache, &serial).err().map(|e| e.to_string());
            assert!(
                err.as_deref()
                    .is_some_and(|e| e.contains("state.json") && !e.contains("running jq")),
                "{text}: {err:?}"
            );
            fs::remove_dir_all(cache).unwrap();
        }
    }

    #[test]
    fn every_record_is_listed_by_serial() {
        let (cache, _) = cache_with(&render(&record()));
        let other = Serial::new("S0").unwrap();
        fs::create_dir_all(phone_dir(&cache, &other)).unwrap();
        write(&cache, &other, &record().with_state(State::NuraInProgress)).unwrap();
        let listed: Vec<String> = all(&cache)
            .unwrap()
            .iter()
            .map(|(s, r)| format!("{s} {}", r.state.name()))
            .collect();
        assert_eq!(listed, ["S0 nura-in-progress", "S1 nura-installed"]);
        assert!(all(&cache.join("nowhere")).unwrap().is_empty());
        fs::remove_dir_all(cache).unwrap();
    }
}
