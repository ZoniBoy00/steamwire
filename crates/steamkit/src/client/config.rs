use std::sync::Arc;
use std::time::Duration;

use crate::discovery::{MemoryServerListProvider, ServerList};
use crate::enums::{EClientPersonaStateFlag, EUniverse};
use crate::net::{CmServer, NetworkListener, ProtocolType, ProtocolTypes};
use crate::util::hardware::{MachineInfoProvider, SystemMachineInfoProvider};
use crate::webapi::WebApi;

use super::DEFAULT_TIMEOUT;

/// Client configuration (SteamKit2's `SteamConfiguration`).
#[derive(Clone, Debug)]
pub struct Config {
    /// The universe to connect to.
    pub universe: EUniverse,
    /// The transports allowed when picking a server from the list. TCP and
    /// WebSocket by default, as in SteamKit2; when a server allows several,
    /// WebSocket is used, then TCP, then UDP.
    pub protocols: ProtocolTypes,
    /// The cell ID used for the server directory and logon.
    pub cell_id: u32,
    /// Connect to this server instead of using the server list.
    pub server: Option<CmServer>,
    /// The server list, shared by every connection made with this config.
    pub server_list: Arc<ServerList>,
    /// The Web API client.
    pub web_api: WebApi,
    /// How long connecting to a server may take (SteamKit2's
    /// `ConnectionTimeout`).
    pub connection_timeout: Duration,
    /// How long a job waits for its reply; each `JobHeartbeat` starts the
    /// wait again (SteamKit2's `AsyncJob.Timeout`, 10 seconds).
    pub job_timeout: Duration,
    /// How many servers to try before giving up.
    pub connect_attempts: usize,
    /// The persona info requested for friends by default.
    pub persona_state_flags: EClientPersonaStateFlag,
    /// Sees every message, unencrypted; for debugging (see
    /// [`NetHookListener`](crate::net::NetHookListener)).
    pub network_listener: Option<Arc<dyn NetworkListener>>,
    /// Where the machine ID sent at logon comes from; the real hardware by
    /// default. Steam uses it to recognise the device.
    pub machine_info_provider: Arc<dyn MachineInfoProvider>,
}

impl Default for Config {
    fn default() -> Self {
        let web_api = WebApi::new();
        Self {
            universe: EUniverse::Public,
            protocols: ProtocolType::Tcp | ProtocolType::WebSocket,
            cell_id: 0,
            server: None,
            server_list: Arc::new(ServerList::new(Arc::new(MemoryServerListProvider::default()), web_api.clone(), 0)),
            web_api,
            connection_timeout: Duration::from_secs(5),
            job_timeout: DEFAULT_TIMEOUT,
            connect_attempts: 5,
            persona_state_flags: EClientPersonaStateFlag::PlayerName
                | EClientPersonaStateFlag::Presence
                | EClientPersonaStateFlag::SourceID
                | EClientPersonaStateFlag::GameExtraInfo
                | EClientPersonaStateFlag::LastSeen,
            network_listener: None,
            machine_info_provider: Arc::new(SystemMachineInfoProvider),
        }
    }
}
