//! CM server discovery (SteamKit2's `SmartCMServerList` and server list
//! providers).
//!
//! A [`ServerList`] is shared by every connection made with the same
//! [`Config`](crate::Config). It loads servers from a [`ServerListProvider`]
//! cache or the Steam directory, hands out the best candidate, and remembers
//! servers that failed so the next connection tries another one.

mod providers;

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub use providers::{FileServerListProvider, MemoryServerListProvider, NullServerListProvider, ServerListProvider};

use crate::error::Result;
use crate::net::{CmServer, ProtocolType, ProtocolTypes};
use crate::webapi::WebApi;

/// Fallback WebSocket server when nothing else is available.
pub const DEFAULT_SERVER_WEBSOCKET: &str = "cmp1-sea1.steamserver.net:443";
/// Fallback TCP server when nothing else is available.
pub const DEFAULT_SERVER_TCP: &str = "ext1-sea1.steamserver.net:27017";

/// Whether a server worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerQuality {
    /// The connection worked.
    Good,
    /// The connection failed or the server asked us to go elsewhere.
    Bad,
}

#[derive(Debug, Clone)]
struct ServerInfo {
    server: CmServer,
    last_bad: Option<SystemTime>,
}

#[derive(Debug)]
struct State {
    servers: Vec<ServerInfo>,
    last_refresh: Option<SystemTime>,
}

/// A list of CM servers with quality tracking.
#[derive(Debug)]
pub struct ServerList {
    state: Mutex<State>,
    provider: Arc<dyn ServerListProvider>,
    web_api: WebApi,
    cell_id: u32,
    allow_directory_fetch: bool,
    /// How old the list may get before it's refreshed from the directory.
    pub refresh_after: Duration,
    /// How long a bad mark lasts.
    pub bad_memory: Duration,
}

impl ServerList {
    /// A list that caches servers in `provider` and refreshes them from the
    /// directory with `web_api`.
    pub fn new(provider: Arc<dyn ServerListProvider>, web_api: WebApi, cell_id: u32) -> Self {
        Self {
            state: Mutex::new(State { servers: Vec::new(), last_refresh: None }),
            provider,
            web_api,
            cell_id,
            allow_directory_fetch: true,
            refresh_after: Duration::from_secs(7 * 24 * 3600),
            bad_memory: Duration::from_secs(5 * 60),
        }
    }

    /// A list with exactly these servers that never asks the directory.
    pub fn fixed(servers: Vec<CmServer>) -> Self {
        let list =
            Self { allow_directory_fetch: false, ..Self::new(Arc::new(NullServerListProvider), WebApi::new(), 0) };
        list.replace(servers, false, Some(SystemTime::now()));
        list
    }

    /// Whether the list may query the Steam directory.
    pub fn set_allow_directory_fetch(&mut self, allow: bool) {
        self.allow_directory_fetch = allow;
    }

    /// Replaces the servers, optionally writing them to the provider.
    pub fn replace(&self, servers: Vec<CmServer>, write_provider: bool, time: Option<SystemTime>) {
        let mut distinct: Vec<CmServer> = Vec::with_capacity(servers.len());
        for s in servers {
            if !distinct.contains(&s) {
                distinct.push(s);
            }
        }
        if write_provider {
            self.provider.update(&distinct);
        }
        let mut state = self.state.lock().unwrap();
        state.last_refresh = time;
        state.servers = distinct.into_iter().map(|server| ServerInfo { server, last_bad: None }).collect();
    }

    /// All servers, in list order.
    pub fn servers(&self) -> Vec<CmServer> {
        self.state.lock().unwrap().servers.iter().map(|s| s.server.clone()).collect()
    }

    fn is_stale(&self, time: Option<SystemTime>) -> bool {
        match time {
            None => true,
            Some(t) => SystemTime::now().duration_since(t).unwrap_or_default() >= self.refresh_after,
        }
    }

    /// Loads the list if it's empty or stale: from the directory when stale,
    /// otherwise from the provider, falling back to the default servers.
    pub async fn ensure_loaded(&self) -> Result<()> {
        let (empty, stale) = {
            let state = self.state.lock().unwrap();
            (state.servers.is_empty(), self.is_stale(state.last_refresh))
        };
        if !empty && !stale {
            return Ok(());
        }

        let provider_time = self.provider.last_refresh();
        let mut tried_directory = false;
        if (stale && !empty || self.is_stale(provider_time)) && self.allow_directory_fetch {
            tried_directory = true;
            match self.web_api.cm_servers(self.cell_id, None, None).await {
                Ok(list) if !list.is_empty() => {
                    tracing::debug!(count = list.len(), "server list from the directory");
                    self.replace(list, true, Some(SystemTime::now()));
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("directory fetch failed: {e}"),
            }
        }

        let cached = self.provider.fetch();
        if !cached.is_empty() {
            tracing::debug!(count = cached.len(), "server list from the provider");
            self.replace(cached, false, provider_time);
            return Ok(());
        }

        if self.allow_directory_fetch && !tried_directory {
            if let Ok(list) = self.web_api.cm_servers(self.cell_id, None, None).await {
                if !list.is_empty() {
                    self.replace(list, true, Some(SystemTime::now()));
                    return Ok(());
                }
            }
        }

        tracing::debug!("falling back to the default servers");
        self.replace(
            vec![CmServer::websocket(DEFAULT_SERVER_WEBSOCKET), CmServer::tcp(DEFAULT_SERVER_TCP)],
            false,
            None,
        );
        Ok(())
    }

    /// Forgets bad marks older than [`bad_memory`](Self::bad_memory).
    pub fn reset_old_scores(&self) {
        let cutoff = SystemTime::now() - self.bad_memory;
        for info in &mut self.state.lock().unwrap().servers {
            if info.last_bad.is_some_and(|t| t < cutoff) {
                info.last_bad = None;
            }
        }
    }

    /// Forgets every bad mark.
    pub fn reset_bad_servers(&self) {
        for info in &mut self.state.lock().unwrap().servers {
            info.last_bad = None;
        }
    }

    /// Marks a server. Bad marks apply to every endpoint on the same host.
    pub fn mark(&self, server: &CmServer, quality: ServerQuality) -> bool {
        let mut state = self.state.lock().unwrap();
        let host = server.host();
        let mut any = false;
        for info in &mut state.servers {
            let hit = match quality {
                ServerQuality::Good => info.server.endpoint == server.endpoint,
                ServerQuality::Bad => info.server.host() == host,
            };
            if hit {
                any = true;
                info.last_bad = match quality {
                    ServerQuality::Good => None,
                    ServerQuality::Bad => Some(SystemTime::now()),
                };
            }
        }
        any
    }

    /// The servers for `protocols`, best first: never-bad servers in list
    /// order, then bad ones by how long ago they failed. Like SteamKit2,
    /// TCP (`netfilter`) servers also serve UDP on the same endpoint; each
    /// candidate uses the protocol [`ProtocolTypes::preferred`] picks from
    /// what both the server and `protocols` allow.
    pub fn candidates(&self, protocols: impl Into<ProtocolTypes>) -> Vec<CmServer> {
        let protocols = protocols.into();
        self.reset_old_scores();
        let state = self.state.lock().unwrap();
        let mut list: Vec<(&ServerInfo, ProtocolType)> = state
            .servers
            .iter()
            .filter_map(|s| Some((s, s.server.protocols().intersection(protocols).preferred()?)))
            .collect();
        list.sort_by_key(|(s, _)| s.last_bad);
        list.into_iter().map(|(s, protocol)| CmServer { endpoint: s.server.endpoint.clone(), protocol }).collect()
    }

    /// The best server for `protocols` (SteamKit2's `GetNextServerCandidate`).
    pub fn next_server(&self, protocols: impl Into<ProtocolTypes>) -> Option<CmServer> {
        self.candidates(protocols).into_iter().next()
    }

    /// When the list was last loaded from the directory or the provider.
    pub fn last_refresh(&self) -> Option<SystemTime> {
        self.state.lock().unwrap().last_refresh
    }

    /// Reloads the list from the directory now, whatever its age (SteamKit2's
    /// `ForceRefreshServerList`). Without directory access, or if the
    /// directory has nothing, the list is reloaded as in
    /// [`ensure_loaded`](Self::ensure_loaded).
    pub async fn force_refresh(&self) -> Result<()> {
        if self.allow_directory_fetch {
            match self.web_api.cm_servers(self.cell_id, None, None).await {
                Ok(list) if !list.is_empty() => {
                    self.replace(list, true, Some(SystemTime::now()));
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("directory fetch failed: {e}"),
            }
        }
        self.state.lock().unwrap().servers.clear();
        self.ensure_loaded().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn marks_and_orders() {
        let list = ServerList::fixed(vec![
            CmServer::websocket("a.example:443"),
            CmServer::websocket("b.example:443"),
            CmServer::tcp("a.example:27017"),
            CmServer::tcp("c.example:27017"),
        ]);
        list.ensure_loaded().await.unwrap();
        assert_eq!(list.next_server(ProtocolType::WebSocket).unwrap().endpoint, "a.example:443");

        assert!(list.mark(&CmServer::websocket("a.example:443"), ServerQuality::Bad));
        // Bad marks apply to the whole host, so a.example's TCP endpoint too.
        assert_eq!(list.next_server(ProtocolType::WebSocket).unwrap().endpoint, "b.example:443");
        assert_eq!(list.next_server(ProtocolType::Tcp).unwrap().endpoint, "c.example:27017");

        list.mark(&CmServer::websocket("a.example:443"), ServerQuality::Good);
        assert_eq!(list.next_server(ProtocolType::WebSocket).unwrap().endpoint, "a.example:443");
        assert_eq!(list.candidates(ProtocolTypes::ALL).len(), 4);

        // TCP servers also serve UDP; WebSocket is preferred when both fit.
        let udp = list.candidates(ProtocolType::Udp);
        assert_eq!(udp.len(), 2);
        assert!(udp.iter().all(|s| s.protocol == ProtocolType::Udp && s.endpoint.ends_with(":27017")));
        let both = list.candidates(ProtocolTypes::TCP | ProtocolTypes::WEB_SOCKET);
        assert_eq!(both.iter().filter(|s| s.protocol == ProtocolType::WebSocket).count(), 2);
    }

    #[tokio::test]
    async fn fallback_without_directory() {
        let mut list = ServerList::new(Arc::new(NullServerListProvider), WebApi::new(), 0);
        list.set_allow_directory_fetch(false);
        list.ensure_loaded().await.unwrap();
        assert_eq!(list.servers().len(), 2);
    }
}
