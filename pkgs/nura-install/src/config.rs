//! Where the index address, cache, tools and limits come from. The shipped binary takes them from build-time
//! values and constants; only a `test-hooks` build reads `NURA_INSTALL_*` from the environment.
use crate::fail::Result;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(feature = "test-hooks")]
fn hook(var: &str) -> Option<OsString> {
    std::env::var_os(var)
}

#[cfg(not(feature = "test-hooks"))]
fn hook(_var: &str) -> Option<OsString> {
    None
}

fn hook_number(var: &str) -> Result<Option<u64>> {
    hook(var)
        .map(|v| {
            v.to_string_lossy()
                .parse()
                .map_err(|_| format!("{var} must be a number, not {v:?}").into())
        })
        .transpose()
}

pub fn nura_index() -> String {
    hook("NURA_INSTALL_NURA_INDEX")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "https://images.nura.eco/bpo/index.json".to_string())
}

/// The cache directory the build set, else the per-user data directory:
/// `$XDG_DATA_HOME/nura-install`, or `~/.local/share/nura-install`.
pub fn cache_dir() -> Result<PathBuf> {
    if let Some(dir) =
        hook("NURA_INSTALL_CACHE").or_else(|| option_env!("NURA_INSTALL_CACHE").map(OsString::from))
    {
        return Ok(PathBuf::from(dir));
    }
    data_home(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
        .map(|dir| dir.join("nura-install"))
        .ok_or_else(|| {
            "neither XDG_DATA_HOME nor HOME is an absolute path, so there is nowhere to keep the cache"
                .into()
        })
}

/// The XDG base directory rule: a relative or unset `XDG_DATA_HOME` falls back to `~/.local/share`.
fn data_home(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |v: OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    xdg.and_then(absolute)
        .or_else(|| home.and_then(absolute).map(|h| h.join(".local/share")))
}

fn tool(var: &str, default: &str) -> OsString {
    hook(var).unwrap_or_else(|| default.into())
}

pub fn adb_bin() -> OsString {
    tool("NURA_INSTALL_ADB", "adb")
}

pub fn fastboot_bin() -> OsString {
    tool("NURA_INSTALL_FASTBOOT", "fastboot")
}

pub fn curl_bin() -> OsString {
    tool("NURA_INSTALL_CURL", "curl")
}

pub fn ssh_bin() -> OsString {
    tool("NURA_INSTALL_SSH", "ssh")
}

pub fn ssh_keyscan_bin() -> OsString {
    tool("NURA_INSTALL_SSH_KEYSCAN", "ssh-keyscan")
}

pub fn ssh_keygen_bin() -> OsString {
    tool("NURA_INSTALL_SSH_KEYGEN", "ssh-keygen")
}

pub fn ip_bin() -> OsString {
    tool("NURA_INSTALL_IP", "ip")
}

/// Nura's USB network: the phone serves 172.16.42.1 and gives the computer 172.16.42.2.
pub fn phone_addr() -> String {
    hook("NURA_INSTALL_PHONE_ADDR")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "172.16.42.1:22".to_string())
}

pub fn local_addr() -> String {
    hook("NURA_INSTALL_LOCAL_ADDR")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| "172.16.42.2".to_string())
}

pub fn ip_forward_file() -> PathBuf {
    hook("NURA_INSTALL_IP_FORWARD_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc/sys/net/ipv4/ip_forward"))
}

pub fn allow_file_urls() -> bool {
    hook("NURA_INSTALL_ALLOW_FILE_URLS").is_some_and(|v| v == "1")
}

pub fn allow_piped_input() -> bool {
    hook("NURA_INSTALL_ALLOW_PIPED_INPUT").is_some_and(|v| v == "1")
}

/// How long Enter is ignored after a menu is drawn; fixed in the shipped build.
pub fn security_delay() -> Result<Duration> {
    Ok(Duration::from_millis(
        hook_number("NURA_INSTALL_SECURITY_DELAY_MS")?.unwrap_or(3000),
    ))
}

/// How long Enter is ignored after each move of a menu's cursor; fixed in the shipped build.
pub fn security_delay_after_move() -> Result<Duration> {
    Ok(Duration::from_millis(
        hook_number("NURA_INSTALL_SECURITY_DELAY_AFTER_MOVE_MS")?.unwrap_or(1000),
    ))
}

pub fn needed_gib() -> Result<u64> {
    Ok(hook_number("NURA_INSTALL_NEEDED_GIB")?.unwrap_or(12))
}

/// The owner's part (the phone showing up over adb or in fastboot) has no wait limit:
/// Ctrl-C ends it. Only the tests set one.
pub fn wait_limit() -> Result<Option<Duration>> {
    Ok(hook_number("NURA_INSTALL_WAIT_SECONDS")?.map(Duration::from_secs))
}

/// The wait for sshd on Nura has no limit either: `skip` or Ctrl-C ends it.
pub fn copy_wait_limit() -> Result<Option<Duration>> {
    Ok(hook_number("NURA_INSTALL_COPY_WAIT_SECONDS")?.map(Duration::from_secs))
}

/// The longest one ssh command to the phone may take.
pub fn ssh_timeout() -> Result<Duration> {
    Ok(Duration::from_secs(
        hook_number("NURA_INSTALL_SSH_TIMEOUT_SECONDS")?.unwrap_or(600),
    ))
}

/// How often a long step writes a plain progress line to the transcript.
pub fn progress_every() -> Duration {
    Duration::from_secs(
        hook_number("NURA_INSTALL_PROGRESS_SECONDS")
            .ok()
            .flatten()
            .unwrap_or(30),
    )
}

/// `slow` writes are the multi-GiB `flash userdata`.
pub fn write_timeout(slow: bool) -> Result<Duration> {
    let default = if slow { 30 * 60 } else { 5 * 60 };
    Ok(Duration::from_secs(
        hook_number("NURA_INSTALL_WRITE_TIMEOUT_SECONDS")?.unwrap_or(default),
    ))
}

#[cfg(all(test, not(feature = "test-hooks")))]
mod tests {
    use super::*;

    #[test]
    fn the_cache_follows_the_xdg_data_directory() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(
            data_home(os("/x/data"), os("/home/u")),
            Some(PathBuf::from("/x/data"))
        );
        assert_eq!(
            data_home(os("rel"), os("/home/u")),
            Some(PathBuf::from("/home/u/.local/share"))
        );
        assert_eq!(
            data_home(None, os("/home/u")),
            Some(PathBuf::from("/home/u/.local/share"))
        );
        assert_eq!(data_home(None, os("rel")), None);
        assert_eq!(data_home(None, None), None);
    }

    #[test]
    fn exported_overrides_do_nothing_in_the_shipped_build() {
        for var in [
            "NURA_INSTALL_NURA_INDEX",
            "NURA_INSTALL_SECURITY_DELAY_MS",
            "NURA_INSTALL_SECURITY_DELAY_AFTER_MOVE_MS",
            "NURA_INSTALL_CACHE",
            "NURA_INSTALL_ADB",
            "NURA_INSTALL_FASTBOOT",
            "NURA_INSTALL_CURL",
            "NURA_INSTALL_NEEDED_GIB",
            "NURA_INSTALL_WAIT_SECONDS",
            "NURA_INSTALL_WRITE_TIMEOUT_SECONDS",
            "NURA_INSTALL_SSH",
            "NURA_INSTALL_SSH_KEYSCAN",
            "NURA_INSTALL_SSH_KEYGEN",
            "NURA_INSTALL_IP",
            "NURA_INSTALL_PHONE_ADDR",
            "NURA_INSTALL_LOCAL_ADDR",
            "NURA_INSTALL_IP_FORWARD_FILE",
            "NURA_INSTALL_COPY_WAIT_SECONDS",
            "NURA_INSTALL_SSH_TIMEOUT_SECONDS",
        ] {
            std::env::set_var(var, "1");
        }
        std::env::set_var("NURA_INSTALL_ALLOW_FILE_URLS", "1");
        std::env::set_var("NURA_INSTALL_ALLOW_PIPED_INPUT", "1");
        assert_eq!(adb_bin(), "adb");
        assert_eq!(fastboot_bin(), "fastboot");
        assert_eq!(curl_bin(), "curl");
        assert_eq!(ssh_bin(), "ssh");
        assert_eq!(ssh_keyscan_bin(), "ssh-keyscan");
        assert_eq!(ssh_keygen_bin(), "ssh-keygen");
        assert_eq!(ip_bin(), "ip");
        assert_eq!(phone_addr(), "172.16.42.1:22");
        assert_eq!(local_addr(), "172.16.42.2");
        assert_eq!(
            ip_forward_file(),
            PathBuf::from("/proc/sys/net/ipv4/ip_forward")
        );
        assert_eq!(copy_wait_limit().unwrap(), None);
        assert_eq!(ssh_timeout().unwrap(), Duration::from_secs(600));
        assert!(!allow_file_urls());
        assert!(!allow_piped_input());
        assert_eq!(security_delay().unwrap(), Duration::from_secs(3));
        assert_eq!(security_delay_after_move().unwrap(), Duration::from_secs(1));
        assert_eq!(needed_gib().unwrap(), 12);
        assert_eq!(wait_limit().unwrap(), None);
        assert_eq!(write_timeout(true).unwrap(), Duration::from_secs(1800));
        assert_eq!(nura_index(), "https://images.nura.eco/bpo/index.json");
        assert_ne!(cache_dir().ok(), Some(PathBuf::from("1")));
    }
}
