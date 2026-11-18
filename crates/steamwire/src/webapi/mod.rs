//! A small client for the Steam Web API (`api.steampowered.com`), and the CM
//! server directory.

mod content;
mod directory;

use std::time::Duration;

use base64::Engine as _;
use prost::Message;
use serde::de::DeserializeOwned;

use crate::enums::EResult;
use crate::error::{Error, Result};
use crate::types::kv::KeyValue;

/// The default Web API host.
pub const DEFAULT_BASE_URL: &str = "https://api.steampowered.com";

/// A Steam Web API client.
///
/// ```no_run
/// # async fn run() -> steamwire::Result<()> {
/// let api = steamwire::webapi::WebApi::new();
/// let info: serde_json::Value = api
///     .get("ISteamWebAPIUtil", "GetServerInfo", 1, &[])
///     .await?;
/// println!("{info}");
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct WebApi {
    http: reqwest::Client,
    base_url: String,
    key: Option<String>,
    timeout: Option<Duration>,
}

impl Default for WebApi {
    fn default() -> Self {
        Self::new()
    }
}

impl WebApi {
    /// A client for the public API without a key.
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("steamkit-rs/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("default reqwest client"),
            base_url: DEFAULT_BASE_URL.to_string(),
            key: None,
            timeout: None,
        }
    }

    /// Sends requests with `http` (SteamKit2's `HttpClientFactory`), for
    /// proxies, custom TLS or a shared connection pool.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Fails requests that take longer than `timeout` (SteamKit2's
    /// `WebAPI.Interface.Timeout`). Without it, the HTTP client's own
    /// timeout applies.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the Web API key sent with every request.
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Sets the base URL (for partner.steam-api.com or a test server).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// An interface such as `ISteamUser`, for calls that return KeyValues
    /// or protobufs (SteamKit2's `WebAPI.GetAsyncInterface`).
    pub fn interface(&self, name: impl Into<String>) -> Interface<'_> {
        Interface { api: self, name: name.into() }
    }

    /// Sends a request with the key (unless `params` has one) and `format`
    /// added; parameters go in the query for GET and the body for POST.
    async fn request(
        &self,
        method: HttpMethod,
        interface: &str,
        func: &str,
        version: u32,
        params: &[(&str, &str)],
        format: &str,
    ) -> Result<reqwest::Response> {
        let mut all: Vec<(&str, &str)> = Vec::with_capacity(params.len() + 2);
        if let Some(key) = self.key.as_deref().filter(|_| !params.iter().any(|(k, _)| *k == "key")) {
            all.push(("key", key));
        }
        match params.iter().find(|(k, _)| *k == "format") {
            Some((_, f)) if *f != format => {
                return Err(Error::Other(format!("unsupported format {f:?}; it must be {format:?} or omitted")));
            }
            Some(_) => {}
            None => all.push(("format", format)),
        }
        all.extend_from_slice(params);

        let url = format!("{}/{interface}/{func}/v{version}/", self.base_url);
        let mut request = match method {
            HttpMethod::Get => self.http.get(url).query(&all),
            HttpMethod::Post => self.http.post(url).form(&all),
        };
        if let Some(timeout) = self.timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(Error::HttpStatus(response.status().as_u16()));
        }
        Ok(response)
    }

    /// Calls a method with GET and parses the JSON response.
    pub async fn get<T: DeserializeOwned>(
        &self,
        interface: &str,
        method: &str,
        version: u32,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let response = self.request(HttpMethod::Get, interface, method, version, params, "json").await?;
        Ok(response.json().await?)
    }

    /// Calls a method with POST (form encoded) and parses the JSON response.
    pub async fn post<T: DeserializeOwned>(
        &self,
        interface: &str,
        method: &str,
        version: u32,
        params: &[(&str, &str)],
    ) -> Result<T> {
        let response = self.request(HttpMethod::Post, interface, method, version, params, "json").await?;
        Ok(response.json().await?)
    }
}

/// The HTTP method of a Web API call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    /// Parameters in the query string.
    Get,
    /// Parameters form encoded in the body.
    Post,
}

/// A protobuf Web API response with its `x-eresult` header.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WebApiResponse<T> {
    /// The result Steam reported (`Invalid` if there was none).
    pub result: EResult,
    /// The response.
    pub body: T,
}

/// A Web API interface (SteamKit2's `WebAPI.AsyncInterface`). List
/// arguments are passed as indexed keys: `("steamids[0]", ...)`.
///
/// ```no_run
/// # async fn run() -> steamwire::Result<()> {
/// use steamwire::webapi::{HttpMethod, WebApi};
///
/// let api = WebApi::new();
/// let info = api.interface("ISteamWebAPIUtil").call(HttpMethod::Get, "GetServerInfo", 1, &[]).await?;
/// println!("{:?}", info["servertime"].as_u64());
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct Interface<'a> {
    api: &'a WebApi,
    name: String,
}

impl Interface<'_> {
    /// Calls `func` and parses the response as KeyValues (`format=vdf`).
    pub async fn call(&self, method: HttpMethod, func: &str, version: u32, args: &[(&str, &str)]) -> Result<KeyValue> {
        let response = self.api.request(method, &self.name, func, version, args, "vdf").await?;
        let text = response.text().await?;
        KeyValue::parse_text(&text).map_err(|e| Error::InvalidData(format!("Web API response is not KeyValues: {e}")))
    }

    /// Calls `func` with a protobuf request (`input_protobuf_encoded`) and
    /// decodes the protobuf response (`format=protobuf_raw`).
    pub async fn call_protobuf<Req: Message, Resp: Message + Default>(
        &self,
        method: HttpMethod,
        func: &str,
        version: u32,
        request: &Req,
    ) -> Result<WebApiResponse<Resp>> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(request.encode_to_vec());
        let args = [("input_protobuf_encoded", encoded.as_str())];
        let response = self.api.request(method, &self.name, func, version, &args, "protobuf_raw").await?;
        let result = response
            .headers()
            .get("x-eresult")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .map_or(EResult::Invalid, EResult);
        let body = Resp::decode(response.bytes().await?)?;
        Ok(WebApiResponse { result, body })
    }
}
