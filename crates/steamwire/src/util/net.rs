//! Conversions between `CMsgIPAddress` and std IP types (SteamKit2's `NetHelpers`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use steamwire_protos::steam::c_msg_ip_address::Ip;
use steamwire_protos::steam::CMsgIpAddress;

/// Converts an IP address to its message form.
pub fn ip_to_msg(ip: IpAddr) -> CMsgIpAddress {
    CMsgIpAddress {
        ip: Some(match ip {
            IpAddr::V4(v4) => Ip::V4(u32::from(v4)),
            IpAddr::V6(v6) => Ip::V6(v6.octets().to_vec()),
        }),
    }
}

/// XORs an address with `0xBAADF00D` (each 4-byte group of an IPv6
/// address too), as the logon's `obfuscated_private_ip` expects (SteamKit2's
/// `NetHelpers.ObfuscatePrivateIP`).
pub fn obfuscate_private_ip(msg: CMsgIpAddress) -> CMsgIpAddress {
    const MASK: u32 = 0xBAAD_F00D;
    let ip = msg.ip.map(|ip| match ip {
        Ip::V4(v4) => Ip::V4(v4 ^ MASK),
        Ip::V6(mut v6) => {
            for (byte, mask) in v6.iter_mut().zip(MASK.to_le_bytes().into_iter().cycle()) {
                *byte ^= mask;
            }
            Ip::V6(v6)
        }
    });
    CMsgIpAddress { ip }
}

/// Converts a message IP address; `None` if it's empty or malformed.
pub fn ip_from_msg(msg: &CMsgIpAddress) -> Option<IpAddr> {
    match msg.ip.as_ref()? {
        Ip::V4(v4) => Some(IpAddr::V4(Ipv4Addr::from(*v4))),
        Ip::V6(v6) => <[u8; 16]>::try_from(v6.as_slice()).ok().map(|b| IpAddr::V6(Ipv6Addr::from(b))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for ip in ["1.2.3.4".parse().unwrap(), "2001:db8::1".parse().unwrap()] {
            assert_eq!(ip_from_msg(&ip_to_msg(ip)), Some(ip));
        }
        assert_eq!(ip_to_msg("1.2.3.4".parse().unwrap()).ip, Some(Ip::V4(0x01020304)));
    }

    #[test]
    fn obfuscation() {
        // IPAddress.None, what SteamKit2 sends over WebSocket.
        let none = obfuscate_private_ip(ip_to_msg("255.255.255.255".parse().unwrap()));
        assert_eq!(none.ip, Some(Ip::V4(0x4552_0FF2)));
        let v6 = obfuscate_private_ip(ip_to_msg("::".parse().unwrap()));
        assert_eq!(v6.ip, Some(Ip::V6([0x0D, 0xF0, 0xAD, 0xBA].repeat(4))));
    }
}
