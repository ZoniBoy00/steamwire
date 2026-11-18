//! The machine ID sent at logon (SteamKit2's `HardwareUtils`): a binary
//! KeyValues `MessageObject` with SHA-1 hashes of the machine GUID (`BB3`),
//! the network adapters' MAC addresses (`FF2`) and the boot disk's ID
//! (`3B3`), read the way the Steam client does on each OS.

use std::fmt;

use sha1::{Digest, Sha1};

use crate::types::kv::KeyValue;

/// Where the machine ID's parts come from (SteamKit2's
/// `IMachineInfoProvider`). A part that's `None` falls back to
/// [`DefaultMachineInfoProvider`].
pub trait MachineInfoProvider: Send + Sync + fmt::Debug {
    /// A stable ID of the machine.
    fn machine_guid(&self) -> Option<Vec<u8>>;
    /// The MAC addresses of the network adapters.
    fn mac_address(&self) -> Option<Vec<u8>>;
    /// An ID of the boot disk.
    fn disk_id(&self) -> Option<Vec<u8>>;
}

/// Placeholders based on the machine name.
#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultMachineInfoProvider;

impl MachineInfoProvider for DefaultMachineInfoProvider {
    fn machine_guid(&self) -> Option<Vec<u8>> {
        Some(format!("{}-SteamKit", machine_name()).into_bytes())
    }

    fn mac_address(&self) -> Option<Vec<u8>> {
        Some(b"SteamKit-MacAddress".to_vec())
    }

    fn disk_id(&self) -> Option<Vec<u8>> {
        Some(b"SteamKit-DiskId".to_vec())
    }
}

/// The real hardware: the registry, adapters and boot disk on Windows,
/// `/etc/machine-id`, `/sys/class/net` and the root UUID on Linux, and the
/// platform serial number on macOS.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemMachineInfoProvider;

impl MachineInfoProvider for SystemMachineInfoProvider {
    fn machine_guid(&self) -> Option<Vec<u8>> {
        platform::machine_guid()
    }

    fn mac_address(&self) -> Option<Vec<u8>> {
        platform::mac_address()
    }

    fn disk_id(&self) -> Option<Vec<u8>> {
        platform::disk_id()
    }
}

/// The machine's name, as .NET's `Environment.MachineName`.
pub fn machine_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_owned()))
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "steamwire".into())
}

/// The first two MAC addresses, each padded to 8 bytes, with zeros for
/// missing ones: 16 bytes, as the Steam client hashes them.
pub fn combined_mac_address(addresses: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    let mut out: Vec<u8> = addresses
        .into_iter()
        .take(2)
        .flat_map(|mut a| {
            a.extend([0, 0]);
            a
        })
        .collect();
    out.resize(16, 0);
    out
}

fn sha1_hex(data: &[u8]) -> String {
    hex::encode(Sha1::digest(data))
}

/// The encoded machine ID for `provider`.
pub fn machine_id(provider: &dyn MachineInfoProvider) -> Vec<u8> {
    let fallback = DefaultMachineInfoProvider;
    let guid = provider.machine_guid().or_else(|| fallback.machine_guid()).unwrap_or_default();
    let mac = provider.mac_address().or_else(|| fallback.mac_address()).unwrap_or_default();
    let disk = provider.disk_id().or_else(|| fallback.disk_id()).unwrap_or_default();
    KeyValue::new_object(
        "MessageObject",
        vec![
            KeyValue::new_string("BB3", sha1_hex(&guid)),
            KeyValue::new_string("FF2", sha1_hex(&mac)),
            KeyValue::new_string("3B3", sha1_hex(&disk)),
        ],
    )
    .to_binary()
}

/// The Windows version, `major.minor.build`, from the registry.
#[cfg(windows)]
pub(crate) fn windows_version() -> Option<(u32, u32, u32)> {
    platform::windows_version()
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WIN32_ERROR};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_INCLUDE_WINS_INFO, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::Networking::WinSock::AF_UNSPEC;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DESCRIPTOR_HEADER,
        STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
    };
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY};
    use windows::Win32::System::IO::DeviceIoControl;

    /// `CTL_CODE(IOCTL_VOLUME_BASE, 0, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

    /// A string value under `HKEY_LOCAL_MACHINE`, from the 64-bit view.
    fn registry_string(subkey: &str, value: &str) -> Option<String> {
        let (subkey, value) = (HSTRING::from(subkey), HSTRING::from(value));
        let flags = RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY;
        let mut size = 0u32;
        // SAFETY: the size query writes only `size`.
        let status = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &subkey, &value, flags, None, None, Some(&mut size)) };
        if status != WIN32_ERROR(0) || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: `buf` holds `size` bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &subkey,
                &value,
                flags,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != WIN32_ERROR(0) {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn machine_guid() -> Option<Vec<u8>> {
        registry_string(r"SOFTWARE\Microsoft\Cryptography", "MachineGuid").map(String::into_bytes)
    }

    /// `major.minor.build`. `CurrentVersion` stays at 6.3 on Windows 10 and
    /// later, so those are recognised by their build number.
    pub fn windows_version() -> Option<(u32, u32, u32)> {
        const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let build: u32 = registry_string(KEY, "CurrentBuildNumber")?.trim().parse().ok()?;
        if build >= 10240 {
            return Some((10, 0, build));
        }
        let version = registry_string(KEY, "CurrentVersion")?;
        let (major, minor) = version.trim().split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?, build))
    }

    /// The MAC addresses of the physical (PCI) adapters, in the order
    /// Windows lists them.
    pub fn mac_address() -> Option<Vec<u8>> {
        let flags = GAA_FLAG_INCLUDE_WINS_INFO | GAA_FLAG_INCLUDE_GATEWAYS;
        let mut size = 16 * 1024u32;
        let mut buf: Vec<u64>;
        loop {
            // u64s keep the buffer aligned for the structs.
            buf = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: `buf` holds `size` bytes.
            let status = unsafe {
                GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buf.as_mut_ptr().cast()), &mut size)
            };
            match status {
                0 => break,
                111 => continue, // ERROR_BUFFER_OVERFLOW: `size` is updated
                _ => return None,
            }
        }
        let mut macs = Vec::new();
        let mut adapter = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !adapter.is_null() {
            // SAFETY: a list built by GetAdaptersAddresses in `buf`.
            let a = unsafe { &*adapter };
            // SAFETY: AdapterName is a NUL-terminated ANSI string.
            let name = unsafe { a.AdapterName.to_string() }.unwrap_or_default();
            let key = format!(
                r"SYSTEM\CurrentControlSet\Control\Network\{{4D36E972-E325-11CE-BFC1-08002BE10318}}\{name}\Connection"
            );
            let physical =
                registry_string(&key, "PnpInstanceID").is_some_and(|id| id.len() > 3 && id.starts_with("PCI"));
            if physical {
                let len = (a.PhysicalAddressLength as usize).min(a.PhysicalAddress.len());
                macs.push(a.PhysicalAddress[..len].to_vec());
            }
            adapter = a.Next;
        }
        Some(super::combined_mac_address(macs))
    }

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: an open handle from CreateFileW.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn open(path: &str) -> Option<Handle> {
        // SAFETY: plain CreateFileW with no security attributes or template.
        let handle = unsafe {
            CreateFileW(
                &HSTRING::from(path),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .ok()?;
        Some(Handle(handle))
    }

    /// `DeviceIoControl` into `out`.
    fn ioctl<T>(
        handle: &Handle,
        code: u32,
        input: Option<&STORAGE_PROPERTY_QUERY>,
        out: *mut T,
        out_size: usize,
    ) -> bool {
        let (input_ptr, input_size) = match input {
            Some(q) => (Some(q as *const _ as *const std::ffi::c_void), size_of::<STORAGE_PROPERTY_QUERY>() as u32),
            None => (None, 0),
        };
        let mut returned = 0u32;
        // SAFETY: `out` points to `out_size` writable bytes.
        unsafe {
            DeviceIoControl(
                handle.0,
                code,
                input_ptr,
                input_size,
                Some(out.cast()),
                out_size as u32,
                Some(&mut returned),
                None,
            )
        }
        .is_ok()
    }

    /// The serial number of the disk holding the Windows directory.
    pub fn disk_id() -> Option<Vec<u8>> {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let volume = root.get(..2)?;
        let volume = open(&format!(r"\\.\{volume}"))?;
        let mut extents = VOLUME_DISK_EXTENTS::default();
        if !ioctl(&volume, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, &mut extents, size_of::<VOLUME_DISK_EXTENTS>())
            || extents.NumberOfDiskExtents != 1
        {
            return None;
        }
        let drive = open(&format!(r"\\.\PhysicalDrive{}", extents.Extents[0].DiskNumber))?;

        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            ..Default::default()
        };
        let mut header = STORAGE_DESCRIPTOR_HEADER::default();
        if !ioctl(
            &drive,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&query),
            &mut header,
            size_of::<STORAGE_DESCRIPTOR_HEADER>(),
        ) {
            return None;
        }
        let size = (header.Size as usize).max(size_of::<STORAGE_DEVICE_DESCRIPTOR>());
        let mut buf = vec![0u64; size.div_ceil(8)];
        if !ioctl(&drive, IOCTL_STORAGE_QUERY_PROPERTY, Some(&query), buf.as_mut_ptr(), size) {
            return None;
        }
        // SAFETY: the buffer is aligned and at least a descriptor long.
        let descriptor = unsafe { &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
        let offset = descriptor.SerialNumberOffset as usize;
        // SAFETY: `buf` is plain bytes.
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, size) };
        if offset == 0 || offset >= size {
            return None;
        }
        let serial = &bytes[offset..];
        let end = serial.iter().position(|&b| b == 0).unwrap_or(serial.len());
        Some(serial[..end].to_vec()).filter(|s| !s.is_empty())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;

    pub fn machine_guid() -> Option<Vec<u8>> {
        ["/var/lib/dbus/machine-id", "/etc/machine-id", "/etc/hostname"].iter().find_map(|f| std::fs::read(f).ok())
    }

    fn mac_addresses(physical_only: bool) -> Vec<Vec<u8>> {
        let Ok(dir) = std::fs::read_dir("/sys/class/net") else { return Vec::new() };
        let mut names: Vec<_> = dir.filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        names.sort();
        let macs: Vec<Vec<u8>> = names
            .iter()
            .filter(|n| !physical_only || Path::new("/sys/class/net").join(n).join("device").exists())
            .filter_map(|n| std::fs::read_to_string(Path::new("/sys/class/net").join(n).join("address")).ok())
            .filter_map(|text| hex::decode(text.trim().replace(':', "")).ok())
            .collect();
        if physical_only && macs.is_empty() {
            return mac_addresses(false);
        }
        macs
    }

    pub fn mac_address() -> Option<Vec<u8>> {
        Some(super::combined_mac_address(mac_addresses(true)))
    }

    pub fn disk_id() -> Option<Vec<u8>> {
        if let Ok(cmdline) = std::fs::read_to_string("/proc/cmdline") {
            for param in ["root=UUID=", "root=PARTUUID="] {
                let found = cmdline.split(' ').find_map(|p| {
                    p.get(..param.len()).filter(|h| h.eq_ignore_ascii_case(param)).map(|_| &p[param.len()..])
                });
                if let Some(value) = found.map(str::trim).filter(|v| !v.is_empty()) {
                    return Some(value.as_bytes().to_vec());
                }
            }
        }
        // The oldest disk UUID link.
        let mut uuids: Vec<_> = std::fs::read_dir("/dev/disk/by-uuid")
            .ok()?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let modified = e.path().symlink_metadata().and_then(|m| m.modified()).ok()?;
                Some((modified, e.file_name().into_string().ok()?))
            })
            .collect();
        uuids.sort();
        uuids.into_iter().next().map(|(_, name)| name.into_bytes())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    /// The platform serial number, from `ioreg`.
    pub fn machine_guid() -> Option<Vec<u8>> {
        let output =
            std::process::Command::new("ioreg").args(["-rd1", "-c", "IOPlatformExpertDevice"]).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().find(|l| l.contains("\"IOPlatformSerialNumber\""))?;
        let serial = line.rsplit('=').next()?.trim().trim_matches('"');
        Some(serial.as_bytes().to_vec()).filter(|s| !s.is_empty())
    }

    /// Like SteamKit2, no MAC addresses on macOS.
    pub fn mac_address() -> Option<Vec<u8>> {
        None
    }

    /// The media UUID of the device mounted at `/`. SteamKit2 reads
    /// `DAMediaUUID` through DiskArbitration; `diskutil` prints the same value
    /// as "Disk / Partition UUID".
    pub fn disk_id() -> Option<Vec<u8>> {
        let output = std::process::Command::new("diskutil").args(["info", "/"]).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().find(|l| l.trim_start().starts_with("Disk / Partition UUID:"))?;
        let uuid = line.split_once(':')?.1.trim();
        Some(uuid.as_bytes().to_vec()).filter(|u| !u.is_empty())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    pub fn machine_guid() -> Option<Vec<u8>> {
        None
    }

    pub fn mac_address() -> Option<Vec<u8>> {
        None
    }

    pub fn disk_id() -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Fixed;

    impl MachineInfoProvider for Fixed {
        fn machine_guid(&self) -> Option<Vec<u8>> {
            Some(b"guid".to_vec())
        }
        fn mac_address(&self) -> Option<Vec<u8>> {
            None
        }
        fn disk_id(&self) -> Option<Vec<u8>> {
            Some(b"disk".to_vec())
        }
    }

    #[test]
    fn message_object() {
        let kv = KeyValue::parse_binary(&machine_id(&Fixed)).unwrap();
        assert_eq!(kv.name, "MessageObject");
        assert_eq!(kv["BB3"].as_str(), Some(sha1_hex(b"guid").as_str()));
        // A missing part falls back to the default provider.
        assert_eq!(kv["FF2"].as_str(), Some(sha1_hex(b"SteamKit-MacAddress").as_str()));
        assert_eq!(kv["3B3"].as_str(), Some(sha1_hex(b"disk").as_str()));
    }

    #[test]
    fn combined_macs() {
        let one = combined_mac_address([vec![1, 2, 3, 4, 5, 6]]);
        assert_eq!(one, [1, 2, 3, 4, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let three = combined_mac_address([vec![1; 6], vec![2; 6], vec![3; 6]]);
        assert_eq!(&three[8..14], &[2; 6]);
        assert_eq!(three.len(), 16);
    }

    /// The system provider reads every part on the machine running the tests.
    #[test]
    fn system_provider_reads_hardware() {
        let p = SystemMachineInfoProvider;
        if cfg!(any(windows, target_os = "linux")) {
            assert!(p.machine_guid().is_some_and(|g| !g.is_empty()), "machine guid");
            assert_eq!(p.mac_address().map(|m| m.len()), Some(16));
        }
        let _ = p.disk_id();
    }
}
