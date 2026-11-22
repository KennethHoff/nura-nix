//! The copy of the per-phone partitions (`persist`, `modemst1`, `modemst2`, `fsg`, `fsc`: the phone's
//! IMEI and calibration), which no download can replace and fastboot cannot read. It runs
//! once Nura boots and the owner has started sshd, over the USB cable's network link only: it refuses
//! when this computer could route that subnet anywhere else, pins Nura's host key per phone on first
//! contact, gives the password through OpenSSH's askpass hook, checks every partition against a hash
//! taken on the phone, and always stops sshd again.
//!
//! ## The askpass contract
//!
//! `ssh` runs `SSH_ASKPASS` for the password. That is this same binary, which answers only when
//! `NURA_INSTALL_ASKPASS` holds the pid of an ancestor of the process (the run that started ssh) and
//! the prompt asks for a password. It is a contract between a parent and the child it starts, not an
//! override: set by hand, the variable matches no ancestor and the binary behaves as usual. The
//! password is Nura's published default and is never in an argument or in the transcript.
use crate::cmd::{self, jq, stdout_of, Sink, Spec};
use crate::config;
use crate::fail::{Fail, Result};
use crate::host::{checked, mkdir, sha256, sync_tree, timestamp};
use crate::phone::Serial;
use crate::prompt;
use crate::prompt::{approve, Entry};
use crate::render::human;
use crate::signals::{self, Copy};
use crate::ui::{self, Gauge, Job};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Nura's published default password for its default user.
const PASSWORD: &str = "147147";
const USER: &str = "user";
pub const PARTITIONS: [&str; 5] = ["persist", "modemst1", "modemst2", "fsg", "fsc"];
const ASKPASS_VAR: &str = "NURA_INSTALL_ASKPASS";
const TRUST_PHRASE: &str = "trust this phone";
const LABELS_DIR: &str = "/dev/disk/by-partlabel";
const SUDO: &str = "sudo -S -p '' ";

/// The pid after the `(comm)` of a /proc/<pid>/stat line.
fn ppid_in(stat: &str) -> Option<u32> {
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(1)?.parse().ok()
}

fn parent_of(pid: u32) -> Option<u32> {
    ppid_in(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// True when `parent` is `want` or one of its few ancestors above this process.
fn descends_from(want: u32, mut pid: u32) -> bool {
    for _ in 0..3 {
        if pid == want {
            return true;
        }
        match parent_of(pid) {
            Some(up) if up > 1 => pid = up,
            _ => return false,
        }
    }
    pid == want
}

/// Answers ssh's password prompt when this process was started by a run of this tool for that
/// purpose; `None` means a normal start.
pub fn askpass() -> Option<ExitCode> {
    let want: u32 = std::env::var(ASKPASS_VAR).ok()?.parse().ok()?;
    // SAFETY: getppid takes no arguments and cannot fail.
    let parent = unsafe { libc::getppid() } as u32;
    if !descends_from(want, parent) {
        return None;
    }
    let prompt = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    if !prompt.contains("password") {
        return Some(ExitCode::from(1));
    }
    println!("{PASSWORD}");
    Some(ExitCode::SUCCESS)
}

fn addr_parts() -> Result<(String, String, SocketAddr)> {
    let addr = config::phone_addr();
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| format!("the phone address {addr:?} is not host:port"))?;
    let socket: SocketAddr = addr
        .parse()
        .map_err(|e| format!("the phone address {addr:?}: {e}"))?;
    Ok((host.to_string(), port.to_string(), socket))
}

fn port_open(addr: &SocketAddr) -> bool {
    TcpStream::connect_timeout(addr, Duration::from_secs(1)).is_ok()
}

/// What the owner's wait ended with.
pub enum Wait {
    Ready,
    Skipped,
}

/// The owner's choice before the copy: do it or skip it.
fn choose_to_wait() -> Result<bool> {
    let go = Entry {
        word: "copy".into(),
        label: "Copy the per-phone partitions".into(),
        about: "start sshd on Nura first (see above)".into(),
    };
    let skip = Entry {
        word: "skip".into(),
        label: "Skip the copy".into(),
        about: "the install stays; run nura-install again later".into(),
    };
    match prompt::menu(&[go, skip])? {
        Some(0) => Ok(true),
        Some(_) => Ok(false),
        None => Err("stopped at your request instead of the copy".into()),
    }
}

/// Asks whether to do the work, then waits for TCP 22 on the phone until it answers. There is no
/// time limit in the shipped build; Ctrl-C ends the wait and the work counts as skipped.
pub fn wait_for_sshd() -> Result<Wait> {
    if !choose_to_wait()? {
        return Ok(Wait::Skipped);
    }
    let (host, port, addr) = addr_parts()?;
    let limit_of = config::copy_wait_limit()?;
    let limit = limit_of.unwrap_or(Duration::MAX);
    signals::set_copy(Copy::Session);
    let waited = ui::run(
        Job::new("sshd on the phone").naming(format!(
            "waiting for TCP {port} on {host}{}; Ctrl-C gives up on the copy",
            crate::render::up_to(limit_of)
        )),
        || {
            let started = Instant::now();
            loop {
                if port_open(&addr) {
                    return Ok((Wait::Ready, format!("{host}:{port} answers")));
                }
                if signals::interrupted() {
                    return Ok((
                        Wait::Skipped,
                        "interrupted, so the copy is skipped".to_string(),
                    ));
                }
                if started.elapsed() >= limit {
                    return Err(format!(
                        "{host}:{port} did not answer within {}; Nura may not be up, or sshd was not started",
                        crate::render::limit(limit.as_secs())
                    )
                    .into());
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        },
    );
    signals::set_copy(Copy::Off);
    if signals::interrupted() {
        signals::clear_interrupt();
    }
    waited
}

/// The USB network interface: the one with this computer's address on the phone's subnet.
fn usb_interface(subnet: &str) -> Result<String> {
    let forward = config::ip_forward_file();
    let value =
        fs::read_to_string(&forward).map_err(|e| format!("reading {}: {e}", forward.display()))?;
    if value.trim() != "0" {
        return Err(format!(
            "net.ipv4.ip_forward is {}: this computer would route between the phone and other networks, so the copy is refused",
            value.trim()
        )
        .into());
    }
    let local = config::local_addr();
    let ip = config::ip_bin();
    let addrs = stdout_of(Command::new(&ip).args(["-j", "addr", "show"]))?;
    let found = jq(
        &[
            "-r",
            "--arg",
            "a",
            &local,
            r#".[] | select(any(.addr_info[]?; .local == $a)) | .ifname"#,
        ],
        &addrs,
    )?;
    let dev = match found.lines().collect::<Vec<_>>().as_slice() {
        [one] => (*one).to_string(),
        [] => {
            return Err(format!(
                "no interface has {local}: the phone's USB network is not up on this computer"
            )
            .into())
        }
        many => {
            return Err(format!(
                "{} interfaces have {local}; refusing to pick one",
                many.len()
            )
            .into())
        }
    };
    let routes = stdout_of(Command::new(&ip).args(["-j", "route", "show"]))?;
    let elsewhere = jq(
        &[
            "-r",
            "--arg",
            "d",
            &dev,
            "--arg",
            "p",
            subnet,
            r#".[] | select((.dst // "") | startswith($p)) | select(.dev != $d) | "\(.dst) on \(.dev)""#,
        ],
        &routes,
    )?;
    if !elsewhere.trim().is_empty() {
        return Err(format!(
            "another interface has a route to the phone's subnet ({}); the copy is refused so the phone is only reached over {dev}",
            elsewhere.trim().replace('\n', ", ")
        )
        .into());
    }
    Ok(dev)
}

fn route_goes_over(dev: &str, phone: &str) -> Result<()> {
    let got = stdout_of(Command::new(config::ip_bin()).args(["-j", "route", "get", phone]))?;
    let via = jq(&["-r", ".[0].dev // \"\""], &got)?;
    if via.trim() == dev {
        Ok(())
    } else {
        Err(format!(
            "this computer would reach {phone} over `{}`, not over {dev}; the copy is refused",
            via.trim()
        )
        .into())
    }
}

fn known_hosts(cache: &Path, serial: &Serial) -> PathBuf {
    cache
        .join("phones")
        .join(serial.as_str())
        .join("known_hosts")
}

/// A new Nura install makes new host keys, so the old trust is dropped.
pub fn forget_host_key(cache: &Path, serial: &Serial) -> Result<()> {
    crate::host::remove(&known_hosts(cache, serial))
}

/// The phone's host key is trusted on first contact only, by a typed phrase, and kept per phone.
fn trust(known: &Path, host: &str, port: &str) -> Result<()> {
    if fs::metadata(known).is_ok_and(|m| m.len() > 0) {
        return Ok(());
    }
    let scan = cmd::capture(
        Command::new(config::ssh_keyscan_bin()).args(["-t", "ed25519", "-p", port, host]),
    )?
    .success()?;
    let keys: Vec<&str> = scan
        .stdout
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .collect();
    let [key] = keys.as_slice() else {
        return Err(format!(
            "the phone gave {} ed25519 host keys, not exactly one",
            keys.len()
        )
        .into());
    };
    let fingerprint = cmd::run(
        Command::new(config::ssh_keygen_bin()).args(["-lf", "-"]),
        Spec {
            stdin: Some(&format!("{key}\n")),
            ..Spec::default()
        },
    )?
    .success()?
    .stdout;
    ui::say(&format!(
        "  The phone's SSH host key: {}",
        fingerprint.trim()
    ));
    ui::say("  Compare it with the one Nura shows (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the phone) if you want to be sure.");
    if !approve(
        TRUST_PHRASE,
        &format!(
            "Trust this key for this phone ({})",
            fingerprint_of(&fingerprint)
        ),
    )? {
        return Err("the phone's host key was not trusted, so nothing was read from it".into());
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(known)
        .map_err(|e| format!("creating {}: {e}", known.display()))?;
    writeln!(file, "{key}").map_err(|e| format!("writing {}: {e}", known.display()))?;
    Ok(())
}

/// The `SHA256:...` part of `ssh-keygen -l` output.
fn fingerprint_of(line: &str) -> &str {
    line.split_whitespace()
        .nth(1)
        .unwrap_or("fingerprint unknown")
}

struct Ssh {
    known: PathBuf,
    host: String,
    port: String,
    local: String,
    timeout: Duration,
}

impl Ssh {
    fn command(&self, remote: &str) -> Result<Command> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("finding this program for ssh's password hook: {e}"))?;
        let mut cmd = Command::new(config::ssh_bin());
        cmd.args(["-F", "/dev/null", "-b", &self.local, "-p", &self.port])
            .args(["-o", "StrictHostKeyChecking=yes"])
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", self.known.display()))
            .args(["-o", "GlobalKnownHostsFile=/dev/null"])
            .args(["-o", "PreferredAuthentications=password"])
            .args(["-o", "PubkeyAuthentication=no"])
            .args(["-o", "NumberOfPasswordPrompts=1"])
            .args(["-o", "ConnectTimeout=15"])
            .arg(format!("{USER}@{}", self.host))
            .arg(remote)
            .env("SSH_ASKPASS", exe)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env(ASKPASS_VAR, std::process::id().to_string());
        Ok(cmd)
    }

    /// `sudo -S` reads the password from ssh's stdin, which the remote command never uses.
    fn run(&self, remote: &str, sink: Sink) -> Result<cmd::Done> {
        let sudo = remote.starts_with(SUDO);
        let line = format!("{PASSWORD}\n");
        cmd::run(
            &mut self.command(remote)?,
            Spec {
                stdin: sudo.then_some(line.as_str()),
                stdout: sink,
                timeout: Some(self.timeout),
                ..Spec::default()
            },
        )?
        .success()
        .map_err(|e| format!("`{remote}` on the phone: {e}").into())
    }

    fn text(&self, remote: &str) -> Result<String> {
        Ok(self.run(remote, Sink::Capture)?.stdout)
    }
}

fn label_path(name: &str) -> String {
    format!("{LABELS_DIR}/{name}")
}

/// `androidboot.serialno=` out of /proc/cmdline.
fn serial_in(cmdline: &str) -> Option<&str> {
    cmdline
        .split_whitespace()
        .find_map(|w| w.strip_prefix("androidboot.serialno="))
}

fn missing_labels(listing: &str) -> Vec<&'static str> {
    let have: Vec<&str> = listing.lines().map(str::trim).collect();
    PARTITIONS
        .iter()
        .copied()
        .filter(|p| !have.contains(p))
        .collect()
}

/// Exactly one device may carry the partition label, else /dev/disk/by-partlabel/<name> is a guess.
fn one_holder(name: &str, holders: &str) -> Result<()> {
    match holders.lines().filter(|l| !l.trim().is_empty()).count() {
        1 => Ok(()),
        0 => Err(format!("no device on the phone carries the partition label {name}").into()),
        n => Err(format!(
            "{n} devices on the phone carry the partition label {name}; which one is meant is unclear, so nothing was copied"
        )
        .into()),
    }
}

/// The copy must be as long as the partition. A blank partition is kept: its hash already equals
/// the phone's, so the partition itself is blank (the FP6's modemst1 is), not the read.
fn plausible_copy(name: &str, file: &Path, size: u64, partition_size: u64) -> Result<bool> {
    if size != partition_size {
        return Err(format!(
            "{name} on this computer is {size} bytes, the partition on the phone is {partition_size}; the copy was dropped"
        )
        .into());
    }
    all_zero(file)
}

fn all_zero(file: &Path) -> Result<bool> {
    let mut f = fs::File::open(file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("reading {}: {e}", file.display()))?;
        if n == 0 {
            return Ok(true);
        }
        if buf[..n].iter().any(|b| *b != 0) {
            return Ok(false);
        }
    }
}

pub struct Taken {
    pub dir: PathBuf,
    pub time: String,
}

fn check_serial(ssh: &Ssh, serial: &Serial) -> Result<()> {
    ui::run(Job::new("serial on the phone"), || {
        let cmdline = ssh.text("cat /proc/cmdline")?;
        match serial_in(&cmdline) {
            Some(s) if s == serial.as_str() => {
                checked("nura serial", s);
                Ok(((), s.to_string()))
            }
            Some(s) => Err(format!(
                "Nura reports serial {s}, but this phone is {serial}; this is another phone"
            )
            .into()),
            None => Err("Nura's /proc/cmdline has no androidboot.serialno".into()),
        }
    })
}

fn session(ssh: &Ssh, cache: &Path, serial: &Serial) -> Result<Taken> {
    ui::run(
        Job::new("partitions on the phone")
            .naming(format!("looking for the five partitions in {LABELS_DIR}")),
        || {
            let listing = ssh.text(&format!("ls -1 {LABELS_DIR}"))?;
            let missing = missing_labels(&listing);
            if missing.is_empty() {
                checked("partlabels", "all five");
                return Ok(((), "all five are there".to_string()));
            }
            Err(format!(
                "the phone has no partition labelled {}; its labels are: {}",
                missing.join(", "),
                listing
                    .lines()
                    .map(str::trim)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into())
        },
    )?;
    check_serial(ssh, serial)?;
    let time = timestamp()?;
    let dir = cache
        .join("backups")
        .join(serial.as_str())
        .join(format!("partitions-{time}"));
    mkdir(&dir)?;
    let mut manifest =
        format!("nura-install copy of the per-phone partitions\nserial: {serial}\ntime: {time}\n");
    for name in PARTITIONS {
        let dest = dir.join(format!("{name}.img"));
        let line = ui::run(
            Job::new(&format!("copy {name}"))
                .naming(format!(
                    "reading {name} from the phone to {}",
                    dest.display()
                ))
                .gauge(Gauge::written(&dest, None)),
            || {
                let remote = label_path(name);
                let holders = ssh.text(&format!(
                    "grep -lx PARTNAME={name} /sys/class/block/*/uevent || true"
                ))?;
                one_holder(name, &holders)?;
                let size_text = ssh.text(&format!("{SUDO}blockdev --getsize64 {remote}"))?;
                let phone_size: u64 = size_text.trim().parse().map_err(|_| {
                    format!("the phone gave no size for {name}: {:?}", size_text.trim())
                })?;
                let sum = ssh.text(&format!("{SUDO}sha256sum {remote}"))?;
                let phone_sha = sum
                    .split_whitespace()
                    .next()
                    .filter(|s| crate::host::is_sha256(s))
                    .ok_or_else(|| format!("the phone gave no hash for {name}"))?
                    .to_string();
                ssh.run(&format!("{SUDO}dd if={remote} bs=4M"), Sink::File(&dest))
                    .inspect_err(|_| {
                        let _ = fs::remove_file(&dest);
                    })?;
                let host_sha = sha256(&dest)?;
                if host_sha != phone_sha {
                    let _ = fs::remove_file(&dest);
                    return Err(format!(
                        "{name} on this computer hashes to {host_sha}, the phone's hash is {phone_sha}; the copy was dropped (the partition may have changed while it was read)"
                    )
                    .into());
                }
                let size = fs::metadata(&dest).map_or(0, |m| m.len());
                let blank = match plausible_copy(name, &dest, size, phone_size) {
                    Ok(blank) => blank,
                    Err(e) => {
                        let _ = fs::remove_file(&dest);
                        return Err(e);
                    }
                };
                let blank = if blank {
                    ", all zeros on the phone too"
                } else {
                    ""
                };
                Ok((
                    format!("{host_sha}  {size}  {name}.img\n"),
                    format!("{}, its hash equals the phone's{blank}", human(size)),
                ))
            },
        )?;
        manifest.push_str(&line);
    }
    let path = dir.join("manifest.txt");
    fs::write(&path, manifest).map_err(|e| format!("writing {}: {e}", path.display()))?;
    sync_tree(&dir, cache)?;
    Ok(Taken { dir, time })
}

static SSHD_MAY_RUN: AtomicBool = AtomicBool::new(false);

/// The owner starts sshd for the copy; until this run has stopped it again, an interrupt must say so.
pub fn sshd_may_run(yes: bool) {
    SSHD_MAY_RUN.store(yes, Ordering::Relaxed);
}

/// What an interrupt tells the owner when sshd may still be running.
pub fn unstopped_sshd() -> Option<String> {
    SSHD_MAY_RUN
        .load(Ordering::Relaxed)
        .then(|| format!("  {}", manual_stop()))
}

/// What the owner is told about sshd when the copy could not even connect.
fn manual_stop() -> String {
    "sshd may still be running on the phone; stop it there with `sudo systemctl stop sshd`"
        .to_string()
}

const STOPPED: &str = "sshd is stopped again";

fn stop_sshd(ssh: &Ssh, addr: &SocketAddr) -> String {
    let stopped = ui::run(
        Job::new("stop sshd").naming("stopping sshd on the phone"),
        || {
            let command = ssh.run(&format!("{SUDO}systemctl stop sshd"), Sink::Capture);
            let mut open = port_open(addr);
            for _ in 0..5 {
                if !open {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
                open = port_open(addr);
            }
            match (command, open) {
                (_, false) => Ok(((), "the port no longer answers".to_string())),
                (Ok(_), true) => Err("the stop command ran, but the port still answers".into()),
                (Err(e), true) => Err(Fail::from(format!("the stop command failed: {e}"))),
            }
        },
    );
    match stopped {
        Ok(()) => STOPPED.to_string(),
        Err(_) => manual_stop(),
    }
}

/// Takes the copy; the second value says what became of sshd, whatever the first says.
pub fn run(cache: &Path, serial: &Serial) -> (Result<Taken>, String) {
    session_with(cache, serial, |ssh| session(ssh, cache, serial))
}

/// Connects (USB link checks, pinned host key), runs `work`, and always stops sshd again; the second
/// value says what became of sshd, whatever the first says.
fn session_with<T>(
    cache: &Path,
    serial: &Serial,
    work: impl FnOnce(&Ssh) -> Result<T>,
) -> (Result<T>, String) {
    let known = known_hosts(cache, serial);
    let (host, port, addr) = match addr_parts() {
        Ok(parts) => parts,
        Err(e) => return (Err(e), manual_stop()),
    };
    let prefix = host
        .rsplit_once('.')
        .map_or(String::new(), |(a, _)| format!("{a}."));
    let local = config::local_addr();
    let net = ui::run(
        Job::new("USB network").naming(format!(
            "checking that {host} is reached only over the USB cable (no forwarding, no other route)"
        )),
        || {
            let dev = usb_interface(&prefix)?;
            route_goes_over(&dev, &host)?;
            Ok((dev.clone(), format!("{host} over {dev}, bound to {local}")))
        },
    );
    if let Err(e) = net {
        return (Err(e), manual_stop());
    }
    if let Err(e) = trust(&known, &host, &port) {
        return (Err(e), manual_stop());
    }
    let ssh = Ssh {
        known,
        host,
        port,
        local,
        timeout: match config::ssh_timeout() {
            Ok(t) => t,
            Err(e) => return (Err(e), manual_stop()),
        },
    };
    signals::set_copy(Copy::Session);
    let result = work(&ssh);
    signals::set_copy(Copy::Stopping);
    let note = stop_sshd(&ssh, &addr);
    signals::set_copy(Copy::Off);
    sshd_may_run(note != STOPPED);
    if signals::interrupted() {
        if note == STOPPED {
            ui::say(&format!("  {note}"));
        }
        crate::interrupted();
    }
    (result, note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_pid_is_read_past_a_command_name_with_spaces_and_brackets() {
        assert_eq!(ppid_in("123 (ssh) S 45 123 123 0 -1"), Some(45));
        assert_eq!(ppid_in("123 (my (odd) name) R 7 1 1"), Some(7));
        assert_eq!(ppid_in("garbage"), None);
    }

    #[test]
    fn this_process_descends_from_its_parent_and_not_from_a_stranger() {
        let me = std::process::id();
        // SAFETY: getppid takes no arguments and cannot fail.
        let parent = unsafe { libc::getppid() } as u32;
        assert!(descends_from(parent, me));
        assert!(descends_from(me, me));
        assert!(!descends_from(u32::MAX - 1, me));
    }

    #[test]
    fn the_serial_comes_from_the_kernel_command_line() {
        assert_eq!(
            serial_in("quiet androidboot.serialno=ABC123 root=/dev/x"),
            Some("ABC123")
        );
        assert_eq!(serial_in("quiet root=/dev/x"), None);
    }

    #[test]
    fn a_partition_label_must_belong_to_exactly_one_device() {
        assert!(one_holder("fsg", "/sys/class/block/sda5/uevent\n").is_ok());
        assert!(one_holder("fsg", "").is_err());
        let two = "/sys/class/block/sda5/uevent\n/sys/class/block/sdb5/uevent\n";
        assert!(one_holder("fsg", two)
            .unwrap_err()
            .to_string()
            .contains("2 devices"));
    }

    #[test]
    fn a_copy_must_match_the_partition_size_and_a_blank_one_is_reported() {
        let dir = std::env::temp_dir().join(format!("nura-copy-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real");
        let blank = dir.join("blank");
        fs::write(&real, [0u8, 0, 7, 0]).unwrap();
        fs::write(&blank, [0u8; 4]).unwrap();
        assert!(!plausible_copy("x", &real, 4, 4).unwrap());
        assert!(plausible_copy("x", &real, 4, 8).is_err());
        assert!(plausible_copy("x", &blank, 4, 4).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_labels_are_named() {
        assert!(missing_labels("persist\nmodemst1\nmodemst2\nfsg\nfsc\nboot_a\n").is_empty());
        assert_eq!(
            missing_labels("persist\nfsg\n"),
            ["modemst1", "modemst2", "fsc"]
        );
    }
}
