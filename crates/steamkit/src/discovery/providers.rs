//! Server list caches.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::net::{CmServer, ProtocolType};

/// Stores the server list between runs.
pub trait ServerListProvider: Send + Sync + std::fmt::Debug {
    /// The cached servers.
    fn fetch(&self) -> Vec<CmServer>;
    /// Replaces the cached servers.
    fn update(&self, servers: &[CmServer]);
    /// When the cache was last written, if ever.
    fn last_refresh(&self) -> Option<SystemTime>;
}

/// Caches nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullServerListProvider;

impl ServerListProvider for NullServerListProvider {
    fn fetch(&self) -> Vec<CmServer> {
        Vec::new()
    }

    fn update(&self, _servers: &[CmServer]) {}

    fn last_refresh(&self) -> Option<SystemTime> {
        None
    }
}

/// Caches in memory, for the lifetime of the process.
#[derive(Debug, Default)]
pub struct MemoryServerListProvider {
    inner: Mutex<(Vec<CmServer>, Option<SystemTime>)>,
}

impl ServerListProvider for MemoryServerListProvider {
    fn fetch(&self) -> Vec<CmServer> {
        self.inner.lock().unwrap().0.clone()
    }

    fn update(&self, servers: &[CmServer]) {
        *self.inner.lock().unwrap() = (servers.to_vec(), Some(SystemTime::now()));
    }

    fn last_refresh(&self) -> Option<SystemTime> {
        self.inner.lock().unwrap().1
    }
}

/// Caches in a JSON file.
#[derive(Debug, Clone)]
pub struct FileServerListProvider {
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct FileFormat {
    updated_unix: u64,
    servers: Vec<FileEntry>,
}

#[derive(Serialize, Deserialize)]
struct FileEntry {
    endpoint: String,
    protocol: String,
}

impl FileServerListProvider {
    /// A provider backed by `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn read(&self) -> Option<FileFormat> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text)
            .map_err(|e| tracing::debug!(path = %self.path.display(), "bad server list file: {e}"))
            .ok()
    }
}

impl ServerListProvider for FileServerListProvider {
    fn fetch(&self) -> Vec<CmServer> {
        let Some(file) = self.read() else { return Vec::new() };
        file.servers
            .into_iter()
            .filter_map(|e| {
                let protocol = match e.protocol.as_str() {
                    "websocket" => ProtocolType::WebSocket,
                    "tcp" => ProtocolType::Tcp,
                    "udp" => ProtocolType::Udp,
                    _ => return None,
                };
                Some(CmServer { endpoint: e.endpoint, protocol })
            })
            .collect()
    }

    fn update(&self, servers: &[CmServer]) {
        let file = FileFormat {
            updated_unix: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            servers: servers
                .iter()
                .map(|s| FileEntry {
                    endpoint: s.endpoint.clone(),
                    protocol: match s.protocol {
                        ProtocolType::WebSocket => "websocket",
                        ProtocolType::Tcp => "tcp",
                        ProtocolType::Udp => "udp",
                    }
                    .into(),
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&file).expect("serializable");
        if let Err(e) = std::fs::write(&self.path, json) {
            tracing::debug!(path = %self.path.display(), "failed to write server list: {e}");
        }
    }

    fn last_refresh(&self) -> Option<SystemTime> {
        self.read().map(|f| UNIX_EPOCH + Duration::from_secs(f.updated_unix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip() {
        let path = std::env::temp_dir().join(format!("steamkit-servers-{}.json", std::process::id()));
        let provider = FileServerListProvider::new(&path);
        assert!(provider.fetch().is_empty());
        let servers = vec![CmServer::websocket("a:443"), CmServer::tcp("b:27017"), CmServer::udp("c:27017")];
        provider.update(&servers);
        assert_eq!(provider.fetch(), servers);
        assert!(provider.last_refresh().is_some());
        let _ = std::fs::remove_file(path);
    }
}
