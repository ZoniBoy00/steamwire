use steamkit_protos::steam::CContentServerDirectoryServerInfo;

/// HTTP or HTTPS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConnectionProtocol {
    /// Plain HTTP.
    #[default]
    Http,
    /// HTTPS.
    Https,
}

/// A content server (SteamKit2's `CDN.Server`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Server {
    /// HTTP or HTTPS.
    pub protocol: ConnectionProtocol,
    /// The host to connect to.
    pub host: String,
    /// The host to send in the `Host` header.
    pub vhost: String,
    /// The port.
    pub port: u16,
    /// `SteamCache`, `CDN`, ...
    pub server_type: String,
    /// Source ID.
    pub source_id: i32,
    /// The cell.
    pub cell_id: u32,
    /// Load.
    pub load: i32,
    /// Weighted load; lower is better.
    pub weighted_load: f32,
    /// How many entries the client should add for this server.
    pub num_entries: i32,
    /// Only for Steam China.
    pub steam_china_only: bool,
    /// Requests go through this server as a proxy.
    pub use_as_proxy: bool,
    /// The proxy path template, with `%host%` and `%path%`.
    pub proxy_request_path_template: Option<String>,
    /// If set, only these apps may use the server.
    pub allowed_app_ids: Vec<u32>,
    /// Proxy types to bypass.
    pub bypass_proxies_of_type: Vec<String>,
}

impl Server {
    /// A server at a host and port (HTTPS when the port is 443).
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let host = host.into();
        Self {
            protocol: if port == 443 { ConnectionProtocol::Https } else { ConnectionProtocol::Http },
            vhost: host.clone(),
            host,
            port,
            ..Default::default()
        }
    }

    /// Converts a server from `ContentServerDirectory.GetServersForSteamPipe`.
    pub fn from_proto(info: &CContentServerDirectoryServerInfo) -> Self {
        let protocol =
            if info.https_support() == "mandatory" { ConnectionProtocol::Https } else { ConnectionProtocol::Http };
        Self {
            protocol,
            host: info.host().to_string(),
            vhost: info.vhost().to_string(),
            port: if protocol == ConnectionProtocol::Https { 443 } else { 80 },
            server_type: info.r#type().to_string(),
            source_id: info.source_id(),
            cell_id: info.cell_id() as u32,
            load: info.load(),
            weighted_load: info.weighted_load(),
            num_entries: info.num_entries_in_client_list(),
            steam_china_only: info.steam_china_only(),
            use_as_proxy: info.use_as_proxy(),
            proxy_request_path_template: info.proxy_request_path_template.clone(),
            allowed_app_ids: info.allowed_app_ids.clone(),
            bypass_proxies_of_type: info.bypass_proxies_of_type.clone(),
        }
    }

    /// The URL scheme.
    pub fn scheme(&self) -> &'static str {
        match self.protocol {
            ConnectionProtocol::Http => "http",
            ConnectionProtocol::Https => "https",
        }
    }
}

impl std::fmt::Display for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{} ({})", self.host, self.port, self.server_type)
    }
}
