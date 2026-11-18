//! The OS type reported at logon (SteamKit2's `Utils.GetOSType`).

use crate::enums::EOSType;

/// The OS family, for [`os_type_for`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsFamily {
    /// Windows NT; the version is `major.minor.build`.
    Windows,
    /// Linux; the version is the kernel's.
    Linux,
    /// macOS; the version is the product version (`15.1`).
    MacOS,
    /// Anything else.
    Other,
}

/// The OS type of the machine running this code, from its version the way
/// SteamKit2 maps it (`Win11` from build 22000, `Linux6x`, `MacOS15`, ...).
pub fn os_type() -> EOSType {
    let family = if cfg!(windows) {
        OsFamily::Windows
    } else if cfg!(target_os = "linux") {
        OsFamily::Linux
    } else if cfg!(target_os = "macos") {
        OsFamily::MacOS
    } else {
        OsFamily::Other
    };
    match version() {
        Some((major, minor, build)) => os_type_for(family, major, minor, build),
        None => match family {
            OsFamily::Windows => EOSType::WinUnknown,
            OsFamily::Linux => EOSType::LinuxUnknown,
            OsFamily::MacOS => EOSType::MacOSUnknown,
            OsFamily::Other => EOSType::Unknown,
        },
    }
}

/// Maps an OS version to its `EOSType`, as SteamKit2's `Utils.GetOSType`
/// does for `Environment.OSVersion`.
pub fn os_type_for(family: OsFamily, major: u32, minor: u32, build: u32) -> EOSType {
    match family {
        OsFamily::Windows => match (major, minor) {
            (4, _) => EOSType::WinNT,
            (5, 0) => EOSType::Win2000,
            (5, 1) => EOSType::WinXP,
            (5, 2) => EOSType::Win2003,
            (6, 0) => EOSType::WinVista,
            (6, 1) => EOSType::Windows7,
            (6, 2) => EOSType::Windows8,
            (6, 3) => EOSType::Windows81,
            (10, _) if build >= 22000 => EOSType::Win11,
            (10, _) => EOSType::Windows10,
            _ => EOSType::WinUnknown,
        },
        OsFamily::Linux => match (major, minor) {
            (2, 2) => EOSType::Linux22,
            (2, 4) => EOSType::Linux24,
            (2, 6) => EOSType::Linux26,
            (2, _) => EOSType::LinuxUnknown,
            (3, 2) => EOSType::Linux32,
            (3, 5) => EOSType::Linux35,
            (3, 6) => EOSType::Linux36,
            (3, 10) => EOSType::Linux310,
            (3, 16) => EOSType::Linux316,
            (3, 18) => EOSType::Linux318,
            (3, _) => EOSType::Linux3x,
            (4, 1) => EOSType::Linux41,
            (4, 4) => EOSType::Linux44,
            (4, 9) => EOSType::Linux49,
            (4, 14) => EOSType::Linux414,
            (4, 19) => EOSType::Linux419,
            (4, _) => EOSType::Linux4x,
            (5, 4) => EOSType::Linux54,
            (5, 10) => EOSType::Linux510,
            (5, _) => EOSType::Linux5x,
            (6, _) => EOSType::Linux6x,
            (7, _) => EOSType::Linux7x,
            _ => EOSType::LinuxUnknown,
        },
        OsFamily::MacOS => match (major, minor) {
            (10, 11) => EOSType::MacOS1011,
            (10, 12) => EOSType::MacOS1012,
            (10, 13) => EOSType::Macos1013,
            (10, 14) => EOSType::Macos1014,
            (10, 16) => EOSType::MacOS1016,
            (10, 17) => EOSType::MacOS1017,
            (10, 18) => EOSType::MacOS1018,
            (10, 19) => EOSType::MacOS1019,
            (10, 20) => EOSType::MacOS1020,
            (10, 21) => EOSType::MacOS1021,
            (10, 22) => EOSType::MacOS1022,
            (10, 23) => EOSType::MacOS1023,
            (10, _) => EOSType::Macos1015,
            (11, 0) => EOSType::MacOS11,
            (11, _) => EOSType::MacOS111,
            (12, _) => EOSType::MacOS12,
            (13, _) => EOSType::MacOS13,
            (14, _) => EOSType::MacOS14,
            (15, _) => EOSType::MacOS15,
            (26, _) => EOSType::MacOS26,
            (27, _) => EOSType::MacOS27,
            (28, _) => EOSType::MacOS28,
            _ => EOSType::MacOSUnknown,
        },
        OsFamily::Other => EOSType::Unknown,
    }
}

/// `major.minor.build` of the running OS.
fn version() -> Option<(u32, u32, u32)> {
    #[cfg(windows)]
    {
        super::hardware::windows_version()
    }
    #[cfg(target_os = "linux")]
    {
        parse_version(&std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?)
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sw_vers").arg("-productVersion").output().ok()?;
        parse_version(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Parses the leading `major.minor[.patch]` of a version string such as
/// `6.8.0-45-generic`.
#[cfg_attr(windows, allow(dead_code))]
fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim().split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_like_steamwire() {
        assert_eq!(os_type_for(OsFamily::Windows, 10, 0, 26100), EOSType::Win11);
        assert_eq!(os_type_for(OsFamily::Windows, 10, 0, 19045), EOSType::Windows10);
        assert_eq!(os_type_for(OsFamily::Windows, 6, 1, 7601), EOSType::Windows7);
        assert_eq!(os_type_for(OsFamily::Linux, 6, 8, 0), EOSType::Linux6x);
        assert_eq!(os_type_for(OsFamily::Linux, 5, 10, 0), EOSType::Linux510);
        assert_eq!(os_type_for(OsFamily::Linux, 2, 5, 0), EOSType::LinuxUnknown);
        assert_eq!(os_type_for(OsFamily::MacOS, 10, 15, 7), EOSType::Macos1015);
        assert_eq!(os_type_for(OsFamily::MacOS, 11, 0, 0), EOSType::MacOS11);
        assert_eq!(os_type_for(OsFamily::MacOS, 15, 1, 0), EOSType::MacOS15);
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("6.8.0-45-generic\n"), Some((6, 8, 0)));
        assert_eq!(parse_version("15.1"), Some((15, 1, 0)));
        assert_eq!(parse_version("x"), None);
    }

    #[test]
    fn detects_this_machine() {
        assert_ne!(os_type(), EOSType::Unknown);
    }
}
