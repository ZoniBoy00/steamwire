//! The CDN client (SteamKit2's `CDN.Client` and `ClientLancache`).

use std::net::IpAddr;
use std::time::Duration;

use super::chunk;
use super::manifest::{ChunkData, DepotManifest};
use super::server::{ConnectionProtocol, Server};
use crate::error::{Error, Result};
use crate::util::compression::zip_decompress;

/// Manifest format version requested from the CDN.
const MANIFEST_VERSION: u32 = 5;
/// The host a lancache answers for.
const LANCACHE_DOMAIN: &str = "lancache.steamcontent.com";

/// Downloads manifests and chunks from content servers.
#[derive(Clone, Debug)]
pub struct CdnClient {
    http: reqwest::Client,
    /// Time to wait for response headers.
    pub request_timeout: Duration,
    /// Time to wait for a response body.
    pub response_body_timeout: Duration,
    /// Send chunk requests to a local lancache; see
    /// [`CdnClient::detect_lancache`].
    pub use_lancache: bool,
}

impl Default for CdnClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether an address is private (loopback, RFC 1918, IPv6 unique or link
/// local), as a lancache's must be.
pub fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xFE00) == 0xFC00 || (first & 0xFFC0) == 0xFE80
        }
    }
}

/// Builds a request URL, routed through `proxy` when it's a proxy.
pub fn build_url(server: &Server, path: &str, query: Option<&str>, proxy: Option<&Server>) -> String {
    let query = match query.filter(|q| !q.is_empty()) {
        Some(q) if q.starts_with('?') => q.to_string(),
        Some(q) => format!("?{q}"),
        None => String::new(),
    };
    if let Some(proxy) = proxy.filter(|p| p.use_as_proxy) {
        if let Some(template) = &proxy.proxy_request_path_template {
            let path = template.replace("%host%", &server.vhost).replace("%path%", &format!("/{path}"));
            let path = path.trim_start_matches('/');
            return format!("{}://{}:{}/{path}{query}", proxy.scheme(), proxy.vhost, proxy.port);
        }
    }
    format!("{}://{}:{}/{path}{query}", server.scheme(), server.vhost, server.port)
}

impl CdnClient {
    /// A client with SteamKit2's timeouts (10 s for headers, 60 s for bodies).
    pub fn new() -> Self {
        Self::with_http_client(reqwest::Client::new())
    }

    /// A client using the given HTTP client.
    pub fn with_http_client(http: reqwest::Client) -> Self {
        Self {
            http,
            request_timeout: Duration::from_secs(10),
            response_body_timeout: Duration::from_secs(60),
            use_lancache: false,
        }
    }

    /// Turns on [`CdnClient::use_lancache`] if `lancache.steamcontent.com`
    /// resolves to a private address.
    pub async fn detect_lancache(&mut self) -> bool {
        self.use_lancache = match tokio::net::lookup_host((LANCACHE_DOMAIN, 80)).await {
            Ok(addrs) => addrs.map(|a| a.ip()).any(is_private_address),
            Err(_) => false,
        };
        self.use_lancache
    }

    async fn get(&self, request: reqwest::RequestBuilder, expected_len: Option<usize>) -> Result<bytes::Bytes> {
        let response =
            tokio::time::timeout(self.request_timeout, request.send()).await.map_err(|_| Error::Timeout)??;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::HttpStatus(status.as_u16()));
        }
        if let (Some(len), Some(expected)) = (response.content_length(), expected_len) {
            if expected > 0 && len as usize != expected {
                return Err(Error::InvalidData(format!("Content-Length mismatch: got {len}, expected {expected}")));
            }
        }
        let body =
            tokio::time::timeout(self.response_body_timeout, response.bytes()).await.map_err(|_| Error::Timeout)??;
        if let Some(expected) = expected_len.filter(|&e| e > 0) {
            if body.len() != expected {
                return Err(Error::InvalidData(format!(
                    "length mismatch: got {} bytes, expected {expected}",
                    body.len()
                )));
            }
        }
        Ok(body)
    }

    /// Downloads a manifest. With the depot key, file names are decrypted.
    /// `request_code` comes from
    /// [`SteamClient::get_manifest_request_code`](crate::SteamClient::get_manifest_request_code);
    /// 0 omits it.
    #[allow(clippy::too_many_arguments)]
    pub async fn download_manifest(
        &self,
        depot_id: u32,
        manifest_id: u64,
        request_code: u64,
        server: &Server,
        depot_key: Option<&[u8]>,
        proxy: Option<&Server>,
        cdn_auth_token: Option<&str>,
    ) -> Result<DepotManifest> {
        let path = if request_code > 0 {
            format!("depot/{depot_id}/manifest/{manifest_id}/{MANIFEST_VERSION}/{request_code}")
        } else {
            format!("depot/{depot_id}/manifest/{manifest_id}/{MANIFEST_VERSION}")
        };
        let url = build_url(server, &path, cdn_auth_token, proxy);
        let zipped = self.get(self.http.get(url), None).await?;
        let mut manifest = DepotManifest::deserialize(&zip_decompress(&zipped, false)?)?;
        if let Some(key) = depot_key {
            manifest.decrypt_filenames(key)?;
        }
        Ok(manifest)
    }

    /// Downloads a chunk. With the depot key, the chunk is decrypted,
    /// decompressed and checked; without, the raw CDN data is returned.
    pub async fn download_depot_chunk(
        &self,
        depot_id: u32,
        chunk: &ChunkData,
        server: &Server,
        depot_key: Option<&[u8]>,
        proxy: Option<&Server>,
        cdn_auth_token: Option<&str>,
    ) -> Result<Vec<u8>> {
        if chunk.chunk_id.is_empty() {
            return Err(Error::Other("the chunk has no ID".into()));
        }
        let path = format!("depot/{depot_id}/chunk/{}", hex::encode(&chunk.chunk_id));
        let request = if self.use_lancache {
            let query = cdn_auth_token.map(|t| format!("?{}", t.trim_start_matches('?'))).unwrap_or_default();
            // The user agent must match the Steam client for lancache to cache.
            self.http
                .get(format!("http://{LANCACHE_DOMAIN}/{path}{query}"))
                .header(reqwest::header::HOST, &server.host)
                .header(reqwest::header::USER_AGENT, "Valve/Steam HTTP Client 1.0")
        } else {
            self.http.get(build_url(server, &path, cdn_auth_token, proxy))
        };
        let data = self.get(request, Some(chunk.compressed_length as usize)).await?;
        match depot_key {
            Some(key) => chunk::process_to_vec(chunk, &data, key),
            None => Ok(data.into()),
        }
    }
}

impl Server {
    /// Whether this server uses HTTPS.
    pub fn is_https(&self) -> bool {
        self.protocol == ConnectionProtocol::Https
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let server = Server { vhost: "cache1.steamcontent.com".into(), ..Server::new("1.2.3.4", 443) };
        assert_eq!(
            build_url(&server, "depot/1/chunk/ab", Some("token=x"), None),
            "https://cache1.steamcontent.com:443/depot/1/chunk/ab?token=x"
        );
        let proxy = Server {
            use_as_proxy: true,
            proxy_request_path_template: Some("/proxy/%host%%path%".into()),
            ..Server::new("proxy.example", 80)
        };
        assert_eq!(
            build_url(&server, "depot/1/chunk/ab", None, Some(&proxy)),
            "http://proxy.example:80/proxy/cache1.steamcontent.com/depot/1/chunk/ab"
        );
    }

    #[test]
    fn private_addresses() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "::1", "fd00::1", "fe80::1"] {
            assert!(is_private_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.32.0.1", "2001:db8::1"] {
            assert!(!is_private_address(ip.parse().unwrap()), "{ip}");
        }
    }
}
