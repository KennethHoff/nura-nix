//! Where the cache, tools and limits come from. The shipped binary takes them from build-time
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

pub fn needed_gib() -> Result<u64> {
    Ok(hook_number("NURA_INSTALL_NEEDED_GIB")?.unwrap_or(12))
}

/// Waits for the owner's part (the phone showing up over adb or in fastboot) have no limit:
/// Ctrl-C ends them. Only the tests set one.
pub fn wait_limit() -> Result<Option<Duration>> {
    Ok(hook_number("NURA_INSTALL_WAIT_SECONDS")?.map(Duration::from_secs))
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
            "NURA_INSTALL_CACHE",
            "NURA_INSTALL_ADB",
            "NURA_INSTALL_FASTBOOT",
            "NURA_INSTALL_NEEDED_GIB",
            "NURA_INSTALL_WAIT_SECONDS",
        ] {
            std::env::set_var(var, "1");
        }
        assert_eq!(adb_bin(), "adb");
        assert_eq!(fastboot_bin(), "fastboot");
        assert_eq!(needed_gib().unwrap(), 12);
        assert_eq!(wait_limit().unwrap(), None);
        assert_ne!(cache_dir().ok(), Some(PathBuf::from("1")));
    }
}
