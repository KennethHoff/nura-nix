//! The Android phase: read-only questions over adb and the backup.
use crate::cmd::{self, capture, stdout_of, to_file};
use crate::config;
use crate::fail::Result;
use crate::host::{checked, is_sha256, mkdir, sync_tree, timestamp};
use crate::log;
use crate::phone::{Serial, Slot};
use crate::prompt::{self, approve, Entry};
use crate::render::human;
use crate::ui::{self, Gauge, Job};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MIN_BATTERY: u32 = 50;
const CONFIRM_PHRASE: &str = "Confirm";
const NOT_BACKED_UP: &str = "\
NOT in the backup, lost for good:
  - app private data (messenger history too)
  - authenticator seeds, passkeys
  - eSIM profiles";
pub struct Export {
    file: &'static str,
    pub uri: &'static str,
    pub label: &'static str,
}

pub const EXPORTS: [Export; 4] = [
    Export {
        file: "contacts.txt",
        uri: "content://com.android.contacts/data",
        label: "contacts",
    },
    Export {
        file: "sms.txt",
        uri: "content://sms",
        label: "SMS",
    },
    Export {
        file: "call-log.txt",
        uri: "content://call_log/calls",
        label: "call log",
    },
    Export {
        file: "calendar.txt",
        uri: "content://com.android.calendar/events",
        label: "calendar",
    },
];

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

fn line_count(path: &Path) -> Result<usize> {
    Ok(fs::read_to_string(path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?
        .lines()
        .count())
}

/// Maps `./path` to its hash; a hash line that doesn't parse or repeats a path is an error.
pub fn parse_manifest(text: &str) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let (hash, name) = line.split_once("  ").ok_or("a manifest line has no hash")?;
        if hash.starts_with('\\') || !is_sha256(hash) {
            return Err("a manifest line has an unsupported hash or file name".into());
        }
        let name = name.strip_prefix("./").unwrap_or(name);
        if map.insert(name.to_string(), hash.to_string()).is_some() {
            return Err("a manifest repeats a file".into());
        }
    }
    Ok(map)
}

pub fn compare_manifests(
    phone: &BTreeMap<String, String>,
    tar: &BTreeMap<String, String>,
) -> Result<()> {
    let missing = phone.keys().filter(|k| !tar.contains_key(*k)).count();
    let extra = tar.keys().filter(|k| !phone.contains_key(*k)).count();
    let differing = phone
        .iter()
        .filter(|(k, v)| tar.get(*k).is_some_and(|t| t != *v))
        .count();
    if missing + extra + differing > 0 {
        return Err(format!(
            "the backup does not match the phone: {missing} files missing from the tar, {extra} extra, {differing} with a different hash"
        )
        .into());
    }
    Ok(())
}

const EXPORT_ERRORS: [&str; 3] = [
    "Error while accessing",
    "Permission Denial",
    "SecurityException",
];

/// The row count of a `content query`: rows, or the explicit empty result; anything else is an error.
pub fn parse_export(stdout: &str, stderr: &str, exited_ok: bool) -> Result<usize> {
    if !exited_ok || EXPORT_ERRORS.iter().any(|e| stderr.contains(e)) {
        return Err("the content query failed or was denied".into());
    }
    let text = stdout.trim();
    if text == "No result found." {
        return Ok(0);
    }
    if text.starts_with("Row: ") {
        return Ok(stdout.lines().filter(|l| l.starts_with("Row: ")).count());
    }
    Err("the content query printed neither rows nor an explicit empty result".into())
}

fn export(serial: &Serial, dir: &Path, item: &Export) -> Result<()> {
    let rows = ui::run(
        Job::new(&format!("export {}", item.file)).naming(format!(
            "exporting the phone's {} with `content query`",
            item.label
        )),
        || {
            let out =
                capture(adb_on(serial).args(["shell", "content", "query", "--uri", item.uri]))?;
            let rows = parse_export(&out.stdout, &out.stderr, out.ok())
                .map_err(|e| format!("exporting {}: {e}; backup stopped", item.uri))?;
            fs::write(dir.join(item.file), &out.stdout)
                .map_err(|e| format!("writing {}: {e}", item.file))?;
            checked(item.label, &format!("{rows} rows"));
            Ok((rows, format!("{rows} rows")))
        },
    )?;
    if rows == 0 {
        ui::say(&format!("  The phone returned no {} at all.", item.label));
        let label = format!("The phone has no {} (go on)", item.label);
        if !approve(CONFIRM_PHRASE, &label)? {
            return Err(
                format!("stopped at an empty {} export; backup stopped", item.label).into(),
            );
        }
    }
    Ok(())
}

const BLOCK: u64 = 512;
/// toybox tar puts names longer than this into a GNU long-name entry before the entry itself.
const NAME_FIELD: usize = 100;

fn padded(n: u64) -> u64 {
    n.div_ceil(BLOCK) * BLOCK
}

/// The exact length of the stream `tar -cf - -C /sdcard .` writes on the phone, and the number of
/// regular files, from `find . -printf '%M %s %p\n'`: toybox's tar (toys/posix/tar.c) writes per entry a
/// 512-byte header, a long-name entry of 512 bytes plus the name and its NUL padded to 512 when the name
/// is over 100 bytes, the file's data padded to 512 for regular files only, and 1024 zero bytes at the
/// end, with no blocking. Directory names get a trailing `/`. `None` for a line it cannot read, since
/// a wrong total is worse than none.
pub fn parse_listing(text: &str) -> Option<(u64, u64)> {
    let mut files = 0;
    let mut bytes = 2 * BLOCK;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let mut fields = line.splitn(3, ' ');
        let (mode, size, path) = (fields.next()?, fields.next()?, fields.next()?);
        let size: u64 = size.parse().ok()?;
        let kind = mode.chars().next()?;
        if kind == 's' {
            continue;
        }
        let name = path.len() + usize::from(kind == 'd' && !path.ends_with('/'));
        bytes += BLOCK;
        if name > NAME_FIELD {
            bytes += BLOCK + padded(name as u64 + 1);
        }
        if kind == '-' {
            files += 1;
            bytes += padded(size);
        }
    }
    Some((files, bytes))
}

/// What /sdcard holds, read so the next waits can show real totals.
pub struct Sizing {
    pub files: Option<u64>,
    /// The exact length of the tar stream.
    pub tar_bytes: Option<u64>,
}

pub fn size_up(serial: &Serial) -> Sizing {
    let measured = ui::run(
        Job::new("sizing /sdcard")
            .naming("listing /sdcard (every file's size, to know the length of the tar stream)"),
        || {
            let listed = capture(
                adb_on(serial).args(["shell", "cd /sdcard && find . -printf '%M %s %p\\n'"]),
            )
            .ok()
            .filter(cmd::Done::ok)
            .and_then(|o| parse_listing(&o.stdout));
            let detail = match listed {
                None => {
                    "the phone did not say; the waits below show bytes and rate only".to_string()
                }
                Some((files, bytes)) => {
                    checked("sdcard tar bytes", &bytes.to_string());
                    format!("{files} files, {}", human(bytes))
                }
            };
            Ok((
                Sizing {
                    files: listed.map(|l| l.0),
                    tar_bytes: listed.map(|l| l.1),
                },
                detail,
            ))
        },
    );
    measured.unwrap_or(Sizing {
        files: None,
        tar_bytes: None,
    })
}

/// Copies /sdcard and the content-provider data to `<cache>/backups/<serial>/backup-<UTC>/`
/// and checks the tar against hashes taken on the phone.
pub fn backup(cache: &Path, serial: &Serial) -> Result<PathBuf> {
    let dir = cache
        .join("backups")
        .join(serial.as_str())
        .join(format!("backup-{}", timestamp()?));
    mkdir(&dir)?;
    take_backup(cache, serial, &dir)
}

const NO_BACKUP_PHRASE: &str = "no backup";

/// The choice before anything is read from the phone: back up, or skip (which takes a typed phrase,
/// since nothing else stores the data). `true` when the backup is to be taken.
pub fn choose_backup() -> Result<bool> {
    let entries = [
        Entry {
            word: "backup".into(),
            label: "Back up the phone".into(),
            about: "storage, contacts, SMS, call log, calendar".into(),
        },
        Entry {
            word: "skip".into(),
            label: "Skip the backup".into(),
            about: String::new(),
        },
    ];
    match prompt::menu(&entries)? {
        Some(0) => return Ok(true),
        Some(_) => {}
        None => return Err("stopped; nothing was written to the phone".into()),
    }
    log::line("no backup taken: skipped at the owner's request");
    ui::say("\nNO BACKUP will be taken. Going on erases Android and everything stored on the phone, with no copy of it.");
    if approve(
        NO_BACKUP_PHRASE,
        "Go on without a backup (the data is gone for good)",
    )? {
        Ok(false)
    } else {
        Err("stopped; no backup was taken and nothing was written to the phone".into())
    }
}

fn take_backup(cache: &Path, serial: &Serial, dir: &Path) -> Result<PathBuf> {
    let dir = dir.to_path_buf();
    let tar = dir.join("sdcard.tar");
    let manifest = dir.join("manifest.sha256");
    let tar_errors = dir.join("tar-errors.txt");
    let manifest_errors = dir.join("manifest-errors.txt");

    let size = size_up(serial);
    ui::run(
        Job::new("copy /sdcard")
            .naming(format!(
                "streaming the phone's storage to {}",
                tar.display()
            ))
            .gauge(Gauge::written(&tar, size.tar_bytes)),
        || {
            let copied = to_file(
                adb_on(serial).args(["exec-out", "tar", "-cf", "-", "-C", "/sdcard", "."]),
                &tar,
                Some(&tar_errors),
            )?;
            if !copied.ok() {
                return Err(format!(
                    "tar on the phone failed ({} messages, see {})",
                    line_count(&tar_errors)?,
                    tar_errors.display()
                )
                .into());
            }
            let bytes = fs::metadata(&tar).map_or(0, |m| m.len());
            Ok(((), format!("{} copied", human(bytes))))
        },
    )?;
    let hashed_files = size
        .files
        .map_or("every file".to_string(), |n| format!("{n} files"));
    let phone_text = ui::run(
        Job::new("hash /sdcard on the phone")
            .naming(format!("hashing {hashed_files} on the phone"))
            .gauge(Gauge::lines("files", size.files)),
        || {
            let hashed = capture(
                adb_on(serial).args(["shell", "cd /sdcard && find . -type f -exec sha256sum {} +"]),
            )?;
            fs::write(&manifest, &hashed.stdout)
                .map_err(|e| format!("writing {}: {e}", manifest.display()))?;
            fs::write(&manifest_errors, &hashed.stderr)
                .map_err(|e| format!("writing {}: {e}", manifest_errors.display()))?;
            let unreadable = line_count(&manifest_errors)?;
            if !hashed.ok() || unreadable > 0 {
                return Err(format!(
                    "the phone could not read {unreadable} files for hashing (see {}); backup stopped",
                    manifest_errors.display()
                )
                .into());
            }
            let lines = hashed.stdout.lines().count();
            Ok((hashed.stdout, format!("{lines} hashes")))
        },
    )?;
    let phone = parse_manifest(&phone_text)?;
    let tar_size = fs::metadata(&tar).ok().map(|m| m.len());
    ui::run(
        Job::new("verify the tar")
            .naming(format!(
                "checking the tar ({}) against {} hashes",
                tar_size.map_or("size unknown".to_string(), human),
                phone.len()
            ))
            .gauge(Gauge::read(&tar, tar_size)),
        || {
            let listing = stdout_of(Command::new("tar").arg("-xf").arg(&tar).arg(
                "--to-command=printf '%s  %s\\n' \"$(sha256sum | cut -d' ' -f1)\" \"$TAR_FILENAME\"",
            ))?;
            compare_manifests(&phone, &parse_manifest(&listing)?)?;
            Ok(((), format!("all {} files match", phone.len())))
        },
    )?;

    for item in &EXPORTS {
        export(serial, &dir, item)?;
    }
    sync_tree(&dir, cache)?;
    ui::say(&format!("  backup verified: {}", dir.display()));
    Ok(dir)
}

pub fn confirm_losses() -> Result<()> {
    ui::say(&format!("\n{NOT_BACKED_UP}\n"));
    if !approve(CONFIRM_PHRASE, "The screen lock is off (confirm)")? {
        return Err("stopped; nothing was written to the phone".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tar_stream_length_is_headers_padded_data_long_names_and_the_end_blocks() {
        let tree = "drwxrwx--- 4096 .\n-rw-rw---- 5 ./a.txt\n-rw-rw---- 0 ./empty\ndrwxrwx--- 4096 ./sub dir\n-rw-rw---- 6 ./sub dir/b c.txt\n";
        // ./ 512, a.txt 512+512, empty 512, sub dir/ 512, b c.txt 512+512, end 1024
        assert_eq!(parse_listing(tree), Some((3, 4608)));
        let long = format!("drwxrwx--- 4096 .\n-rw-rw---- 1 ./{}\n", "x".repeat(118));
        // ./ 512, long-name entry 512+512, header 512, data 512, end 1024
        assert_eq!(parse_listing(&long), Some((1, 3584)));
        let at_limit = format!("-rw-rw---- 0 {}\n", "x".repeat(100));
        assert_eq!(parse_listing(&at_limit), Some((1, 512 + 1024)));
        assert_eq!(parse_listing(""), Some((0, 1024)));
        assert_eq!(parse_listing("-rw-rw---- big ./a\n"), None);
        assert_eq!(parse_listing("garbage\n"), None);
    }

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

    #[test]
    fn manifest_comparison_counts_and_hashes() {
        let h = |c: char| c.to_string().repeat(64);
        let m = |rows: &[(&str, char)]| {
            let text: String = rows
                .iter()
                .map(|(n, c)| format!("{}  ./{n}\n", h(*c)))
                .collect();
            parse_manifest(&text).unwrap()
        };
        let phone = m(&[("a", '1'), ("b c", '2')]);
        assert!(compare_manifests(&phone, &m(&[("b c", '2'), ("a", '1')])).is_ok());
        assert!(compare_manifests(&phone, &m(&[("a", '1')])).is_err());
        assert!(compare_manifests(&phone, &m(&[("a", '1'), ("b c", '3')])).is_err());
        assert!(compare_manifests(&phone, &m(&[("a", '1'), ("b c", '2'), ("d", '4')])).is_err());
        assert!(parse_manifest(&format!("{}  ./a\n{}  ./a\n", h('1'), h('1'))).is_err());
        assert!(parse_manifest("nothex  ./a\n").is_err());
    }

    #[test]
    fn exports_are_rows_or_an_explicit_empty_result() {
        assert_eq!(
            parse_export("Row: 0 a=1\nRow: 1 a=2\n", "", true).unwrap(),
            2
        );
        assert_eq!(
            parse_export("Row: 0 body=line1\nline2\nRow: 1 a=2\r\n", "", true).unwrap(),
            2
        );
        assert_eq!(parse_export("No result found.\n", "", true).unwrap(), 0);
        assert_eq!(parse_export("No result found.\r\n", "", true).unwrap(), 0);
        assert!(parse_export("", "", true).is_err());
        assert!(parse_export("something else\n", "", true).is_err());
        assert!(parse_export("Error while accessing provider:sms\n", "", true).is_err());
        assert!(parse_export("Row: 0 a=1\n", "", false).is_err());
        assert!(parse_export(
            "Row: 0 a=1\n",
            "java.lang.SecurityException: Permission Denial",
            true
        )
        .is_err());
        assert!(parse_export("Row: 0 a=1\n", "Error while accessing provider", true).is_err());
    }
}
