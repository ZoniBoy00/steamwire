//! CM transports: WebSocket (TLS), and TCP and UDP with channel encryption.
//!
//! A connected transport is a pair of channels: raw outgoing messages go in,
//! raw incoming messages come out. When the socket closes, the incoming
//! channel ends.

use std::time::Duration;

pub mod encryption;
mod envelope;
mod listener;
mod tcp;
pub mod udp;
mod websocket;

pub(crate) use listener::raw_emsg;
pub use listener::{NetHookListener, NetworkListener};

use std::net::IpAddr;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::enums::EUniverse;
use crate::error::{Error, Result};

/// The transport used to reach a CM server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProtocolType {
    /// `VT01` framed TCP with channel encryption.
    Tcp,
    /// WebSocket over TLS (`wss://host:port/cmsocket/`).
    WebSocket,
    /// Steam's reliable UDP protocol with channel encryption.
    Udp,
}

/// A set of [`ProtocolType`]s (SteamKit2's `ProtocolTypes` flags).
///
/// ```
/// use steamwire::net::{ProtocolType, ProtocolTypes};
///
/// let protocols = ProtocolType::Tcp | ProtocolType::WebSocket;
/// assert!(protocols.contains(ProtocolType::Tcp));
/// assert!(!protocols.contains(ProtocolType::Udp));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProtocolTypes(u8);

impl ProtocolTypes {
    /// No protocol.
    pub const NONE: Self = Self(0);
    /// TCP.
    pub const TCP: Self = Self(1);
    /// UDP.
    pub const UDP: Self = Self(2);
    /// WebSocket.
    pub const WEB_SOCKET: Self = Self(4);
    /// Every protocol.
    pub const ALL: Self = Self(7);

    /// The flag bits, as in SteamKit2 (`Tcp = 1`, `Udp = 2`, `WebSocket = 4`).
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether `protocol` is in the set.
    pub const fn contains(self, protocol: ProtocolType) -> bool {
        self.0 & Self::of(protocol).0 != 0
    }

    /// Whether the set is empty.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The protocols both sets have.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The protocol a connection uses when several are allowed: WebSocket,
    /// then TCP, then UDP (SteamKit2's `CMClient.CreateConnection`).
    pub const fn preferred(self) -> Option<ProtocolType> {
        if self.contains(ProtocolType::WebSocket) {
            Some(ProtocolType::WebSocket)
        } else if self.contains(ProtocolType::Tcp) {
            Some(ProtocolType::Tcp)
        } else if self.contains(ProtocolType::Udp) {
            Some(ProtocolType::Udp)
        } else {
            None
        }
    }

    const fn of(protocol: ProtocolType) -> Self {
        match protocol {
            ProtocolType::Tcp => Self::TCP,
            ProtocolType::Udp => Self::UDP,
            ProtocolType::WebSocket => Self::WEB_SOCKET,
        }
    }
}

impl From<ProtocolType> for ProtocolTypes {
    fn from(protocol: ProtocolType) -> Self {
        Self::of(protocol)
    }
}

impl std::ops::BitOr for ProtocolTypes {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOr<ProtocolType> for ProtocolTypes {
    type Output = Self;
    fn bitor(self, rhs: ProtocolType) -> Self {
        self | Self::of(rhs)
    }
}

impl std::ops::BitOr for ProtocolType {
    type Output = ProtocolTypes;
    fn bitor(self, rhs: Self) -> ProtocolTypes {
        ProtocolTypes::of(self) | rhs
    }
}

/// A CM server endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CmServer {
    /// `host:port`.
    pub endpoint: String,
    /// How to talk to it.
    pub protocol: ProtocolType,
}

impl CmServer {
    /// A WebSocket server.
    pub fn websocket(endpoint: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into(), protocol: ProtocolType::WebSocket }
    }

    /// A TCP server.
    pub fn tcp(endpoint: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into(), protocol: ProtocolType::Tcp }
    }

    /// A UDP server.
    pub fn udp(endpoint: impl Into<String>) -> Self {
        Self { endpoint: endpoint.into(), protocol: ProtocolType::Udp }
    }

    /// The protocols the server serves: `netfilter` (TCP) servers also take
    /// UDP on the same endpoint, as in SteamKit2's `ServerRecord`.
    pub fn protocols(&self) -> ProtocolTypes {
        match self.protocol {
            ProtocolType::WebSocket => ProtocolTypes::WEB_SOCKET,
            ProtocolType::Tcp | ProtocolType::Udp => ProtocolTypes::TCP | ProtocolTypes::UDP,
        }
    }

    /// Parses `host:port` (or `[v6]:port`) as a TCP server, like SteamKit2's
    /// `ServerRecord.TryCreateSocketServer`. `None` without a valid port.
    pub fn parse_socket(address: &str) -> Option<Self> {
        let (host, port) = address.rsplit_once(':')?;
        port.parse::<u16>().ok()?;
        (!host.is_empty()).then(|| Self::tcp(address))
    }

    /// The port of the endpoint; 443 for a WebSocket server without one.
    pub fn port(&self) -> Option<u16> {
        match self.endpoint.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(':') => port.parse().ok(),
            _ if self.protocol == ProtocolType::WebSocket => Some(443),
            _ => None,
        }
    }

    /// The host part of the endpoint.
    pub fn host(&self) -> &str {
        match self.endpoint.rsplit_once(':') {
            Some((host, _)) => host.trim_start_matches('[').trim_end_matches(']'),
            None => &self.endpoint,
        }
    }
}

/// A message for the writer task.
pub(crate) enum Outgoing {
    /// Send this message.
    Data(Vec<u8>),
    /// Close the connection.
    Close,
}

/// A connected transport.
pub(crate) struct Connection {
    pub outgoing: mpsc::UnboundedSender<Outgoing>,
    pub incoming: mpsc::UnboundedReceiver<Bytes>,
    /// The local address of the socket; `None` for WebSocket, whose local IP
    /// SteamKit2 doesn't know either (`IPAddress.None`).
    pub local_ip: Option<IpAddr>,
}

pub(crate) async fn connect(
    server: &CmServer,
    universe: EUniverse,
    listener: Option<Arc<dyn NetworkListener>>,
    timeout: Duration,
) -> Result<Connection> {
    tracing::debug!(endpoint = %server.endpoint, protocol = ?server.protocol, "connecting");
    let connect = async {
        match server.protocol {
            ProtocolType::WebSocket => websocket::connect(&server.endpoint).await,
            ProtocolType::Tcp => envelope::wrap(tcp::connect(&server.endpoint).await?, universe, listener).await,
            ProtocolType::Udp => envelope::wrap(udp::connect(&server.endpoint).await?, universe, listener).await,
        }
    };
    tokio::time::timeout(timeout, connect).await.map_err(|_| Error::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_sets() {
        let set = ProtocolTypes::TCP | ProtocolTypes::WEB_SOCKET;
        assert_eq!(set.bits(), 5);
        assert!(set.contains(ProtocolType::Tcp) && !set.contains(ProtocolType::Udp));
        assert_eq!(set.preferred(), Some(ProtocolType::WebSocket));
        assert_eq!(ProtocolTypes::from(ProtocolType::Udp).preferred(), Some(ProtocolType::Udp));
        assert_eq!(set.intersection(ProtocolTypes::UDP), ProtocolTypes::NONE);
        assert!(ProtocolTypes::NONE.is_empty() && ProtocolTypes::NONE.preferred().is_none());
    }

    #[test]
    fn servers() {
        let tcp = CmServer::parse_socket("1.2.3.4:27017").unwrap();
        assert_eq!(tcp.protocol, ProtocolType::Tcp);
        assert_eq!(tcp.port(), Some(27017));
        assert_eq!(tcp.host(), "1.2.3.4");
        assert_eq!(tcp.protocols(), ProtocolTypes::TCP | ProtocolTypes::UDP);
        assert!(CmServer::parse_socket("1.2.3.4").is_none());
        assert!(CmServer::parse_socket("1.2.3.4:http").is_none());
        assert!(CmServer::parse_socket(":27017").is_none());
        assert_eq!(CmServer::parse_socket("[::1]:27017").unwrap().port(), Some(27017));

        let ws = CmServer::websocket("cm.example");
        assert_eq!(ws.port(), Some(443));
        assert_eq!(ws.protocols(), ProtocolTypes::WEB_SOCKET);
    }
}
