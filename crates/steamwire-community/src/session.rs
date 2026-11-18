//! A logged-in session on Steam's websites, made from an access token.
//!
//! The cookies (`steamLoginSecure` and a random `sessionid`) and the
//! renewal of the access token follow SteamHelper-rs's `steam-mobile`
//! (MIT, Copyright (c) 2020 Martin Mariano), which follows
//! node-steam-session.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::future::BoxFuture;
use prost::Message;
use reqwest::header::{COOKIE, LOCATION, REFERER, SET_COOKIE};
use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;
use steamwire::{EResult, SteamClient, SteamId};

use crate::error::{Error, Result};

/// How long before its access token expires a renewing session asks for a
/// new one.
const RENEW_BEFORE: Duration = Duration::from_secs(60 * 60);
/// How long a session waits after a failed renewal before it asks again,
/// using the current token meanwhile.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(30);
/// The least time between two renewals of a short-lived token.
const MIN_RENEWAL_INTERVAL: Duration = Duration::from_secs(30);

/// Where the session sends its requests. The defaults are Steam's; tests
/// point them at a local server.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Urls {
    /// `https://steamcommunity.com`
    pub community: String,
    /// `https://store.steampowered.com`
    pub store: String,
    /// `https://checkout.steampowered.com`
    pub checkout: String,
    /// `https://api.steampowered.com`
    pub api: String,
}

impl Default for Urls {
    fn default() -> Self {
        Self {
            community: "https://steamcommunity.com".into(),
            store: "https://store.steampowered.com".into(),
            checkout: "https://checkout.steampowered.com".into(),
            api: "https://api.steampowered.com".into(),
        }
    }
}

impl Urls {
    /// Every host at one base URL, for tests against a local server.
    pub fn all(base: &str) -> Self {
        let base = base.trim_end_matches('/').to_string();
        Self { community: base.clone(), store: base.clone(), checkout: base.clone(), api: base }
    }
}

/// Something that issues access tokens for the session's account: the Steam
/// client it logged on with, or a test's stand-in.
pub trait TokenIssuer: Send + Sync + 'static {
    /// Asks for a new access token.
    fn issue(&self) -> BoxFuture<'_, Result<String>>;
}

/// Issues access tokens with `Authentication.GenerateAccessTokenForApp` over
/// a logged-on client.
pub struct ClientTokenIssuer {
    client: SteamClient,
    steam_id: SteamId,
    refresh_token: String,
}

impl ClientTokenIssuer {
    /// Uses `client`, logged on as `steam_id`, and the account's refresh
    /// token (the one the client logged on with).
    pub fn new(client: SteamClient, steam_id: SteamId, refresh_token: impl Into<String>) -> Self {
        Self { client, steam_id, refresh_token: refresh_token.into() }
    }
}

impl TokenIssuer for ClientTokenIssuer {
    fn issue(&self) -> BoxFuture<'_, Result<String>> {
        Box::pin(async move {
            let response = self.client.generate_access_token(self.steam_id, &self.refresh_token, false).await?;
            response
                .access_token
                .filter(|t| !t.is_empty())
                .ok_or_else(|| Error::invalid("Steam issued no access token"))
        })
    }
}

struct Token {
    secret: String,
    /// When to ask for a new one; `None` without an issuer.
    renew_at: Option<Instant>,
    expires_at: Option<Instant>,
}

impl Token {
    fn new(secret: String, renewing: bool, now: Instant) -> Self {
        let lasts = token_lifetime(&secret);
        let expires_at = lasts.map(|lasts| now + lasts);
        let renew_at = renewing.then(|| {
            let lasts = lasts.unwrap_or(RENEW_BEFORE * 2);
            now + lasts.saturating_sub(RENEW_BEFORE).max(MIN_RENEWAL_INTERVAL)
        });
        Self { secret, renew_at, expires_at }
    }
}

/// How long a JWT access token lasts, from its own `iat` and `exp` claims
/// (both Steam's clock, so the local clock doesn't matter).
pub(crate) fn token_lifetime(token: &str) -> Option<Duration> {
    let payload = token.split('.').nth(1)?;
    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&json).ok()?;
    let exp = claims["exp"].as_u64()?;
    let iat = claims["iat"].as_u64().or_else(|| claims["nbf"].as_u64())?;
    exp.checked_sub(iat).map(Duration::from_secs)
}

/// The account's access token. Its `Debug` hides it; keep it out of logs.
#[derive(Clone)]
pub struct AccessToken(String);

impl AccessToken {
    /// The token itself.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccessToken(..)")
    }
}

struct Inner {
    http: reqwest::Client,
    urls: Urls,
    steam_id: SteamId,
    session_id: String,
    issuer: Option<Arc<dyn TokenIssuer>>,
    token: tokio::sync::Mutex<Token>,
    /// Cookies Steam set on its answers, such as `webTradeEligibility`.
    cookies: Mutex<Vec<(String, String)>>,
}

/// A logged-in session on steamcommunity.com, the store and the checkout,
/// and an access token for the Web API. Cheap to clone; clones share the
/// session.
///
/// ```no_run
/// # async fn run(client: steamwire::SteamClient, refresh_token: String) -> steamwire_community::Result<()> {
/// use steamwire_community::WebSession;
///
/// // A session that renews its token through the client about daily.
/// let session = WebSession::from_client(&client, refresh_token).await?;
/// println!("logged in: {}", session.is_logged_in().await?);
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct WebSession(Arc<Inner>);

impl std::fmt::Debug for WebSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSession").field("steam_id", &self.0.steam_id).finish_non_exhaustive()
    }
}

/// Settings of a [`WebSession`] before it has a token.
#[derive(Debug)]
pub struct WebSessionBuilder {
    steam_id: SteamId,
    urls: Urls,
    timeout: Duration,
}

impl WebSessionBuilder {
    /// Sends the requests to `urls` instead of Steam's.
    pub fn urls(mut self, urls: Urls) -> Self {
        self.urls = urls;
        self
    }

    /// The timeout of each request (30 seconds by default).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// A session with a fixed access token, which is never renewed: it
    /// lasts as long as the token (about a day).
    pub fn access_token(self, token: impl Into<String>) -> Result<WebSession> {
        let token = Token::new(token.into(), false, Instant::now());
        self.build(None, token)
    }

    /// A session that gets its tokens from `issuer`, and a new one about an
    /// hour before each expires.
    pub async fn issuer(self, issuer: Arc<dyn TokenIssuer>) -> Result<WebSession> {
        let token = Token::new(issuer.issue().await?, true, Instant::now());
        self.build(Some(issuer), token)
    }

    fn build(self, issuer: Option<Arc<dyn TokenIssuer>>, token: Token) -> Result<WebSession> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("steamwire-community/", env!("CARGO_PKG_VERSION")))
            // A redirect to the login page means the session has expired.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(self.timeout)
            .build()?;
        let mut id = [0u8; 12];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut id);
        Ok(WebSession(Arc::new(Inner {
            http,
            urls: self.urls,
            steam_id: self.steam_id,
            // As node-steam-session: 12 random bytes in hex.
            session_id: hex::encode(id),
            issuer,
            token: tokio::sync::Mutex::new(token),
            cookies: Mutex::new(Vec::new()),
        })))
    }
}

impl WebSession {
    /// Starts a session for `steam_id`.
    pub fn builder(steam_id: SteamId) -> WebSessionBuilder {
        WebSessionBuilder { steam_id, urls: Urls::default(), timeout: Duration::from_secs(30) }
    }

    /// A session for the account `client` is logged on as, renewing its
    /// token through the client with `refresh_token` (the token the client
    /// logged on with). Keep the client logged on for the renewals.
    pub async fn from_client(client: &SteamClient, refresh_token: impl Into<String>) -> Result<Self> {
        let steam_id = client.steam_id().ok_or(steamwire::Error::NotConnected)?;
        let issuer = ClientTokenIssuer::new(client.clone(), steam_id, refresh_token);
        Self::builder(steam_id).issuer(Arc::new(issuer)).await
    }

    /// The account's Steam ID.
    pub fn steam_id(&self) -> SteamId {
        self.0.steam_id
    }

    /// The `sessionid` cookie, which Steam's forms also take as a field.
    pub fn session_id(&self) -> &str {
        &self.0.session_id
    }

    pub(crate) fn urls(&self) -> &Urls {
        &self.0.urls
    }

    /// The current access token, renewed first if it is due.
    pub async fn access_token(&self) -> Result<AccessToken> {
        let mut token = self.0.token.lock().await;
        let now = Instant::now();
        if let (Some(issuer), Some(renew_at)) = (&self.0.issuer, token.renew_at) {
            if now >= renew_at {
                match issuer.issue().await {
                    Ok(secret) => *token = Token::new(secret, true, Instant::now()),
                    Err(e) => {
                        if token.expires_at.is_some_and(|expires| now >= expires) {
                            return Err(e);
                        }
                        tracing::warn!("renewing the web access token failed, retrying soon: {e}");
                        token.renew_at = Some(now + RETRY_AFTER_FAILURE);
                    }
                }
            }
        }
        Ok(AccessToken(token.secret.clone()))
    }

    /// Whether Steam still takes the session's cookies: the store's account
    /// page answers without sending to the login page.
    pub async fn is_logged_in(&self) -> Result<bool> {
        let url = format!("{}/account/", self.0.urls.store);
        match self.web(self.0.http.get(url)).await {
            Ok(_) => Ok(true),
            Err(Error::SessionExpired) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn cookie_header(&self, token: &AccessToken) -> String {
        let login = encode_uri_component(&format!("{}||{}", self.0.steam_id.as_u64(), token.expose_secret()));
        let mut header = format!("steamLoginSecure={login}; sessionid={}", self.0.session_id);
        for (name, value) in self.0.cookies.lock().expect("cookies").iter() {
            let _ = write!(header, "; {name}={value}");
        }
        header
    }

    fn keep_cookies(&self, response: &Response) {
        let mut cookies = self.0.cookies.lock().expect("cookies");
        for value in response.headers().get_all(SET_COOKIE) {
            let Some((name, value)) =
                value.to_str().ok().and_then(|c| c.split(';').next()).and_then(|c| c.split_once('='))
            else {
                continue;
            };
            let (name, value) = (name.trim(), value.trim());
            // The session's own cookies stay as they are.
            if matches!(name, "steamLoginSecure" | "sessionid") {
                continue;
            }
            cookies.retain(|(n, _)| n != name);
            if !value.is_empty() && value != "deleted" {
                cookies.push((name.to_string(), value.to_string()));
            }
        }
    }

    /// Sends a request to one of the websites, with the session's cookies.
    async fn web(&self, request: RequestBuilder) -> Result<Response> {
        let token = self.access_token().await?;
        let response = request.header(COOKIE, self.cookie_header(&token)).send().await?;
        self.keep_cookies(&response);
        if response.status().is_redirection() {
            let location = response.headers().get(LOCATION).and_then(|l| l.to_str().ok()).unwrap_or_default();
            if location.contains("/login") || location.contains("lostauth") {
                return Err(Error::SessionExpired);
            }
            return Err(Error::HttpStatus(response.status().as_u16()));
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::SessionExpired);
        }
        Ok(response)
    }

    /// GETs `url` (one of the websites).
    pub(crate) async fn get(&self, url: &str, query: &[(&str, String)]) -> Result<Response> {
        self.web(self.0.http.get(url).query(query)).await
    }

    /// POSTs a form to `url` (one of the websites).
    pub(crate) async fn post(&self, url: &str, form: &[(&str, String)], referer: Option<&str>) -> Result<Response> {
        let mut request = self.0.http.post(url).form(form);
        if let Some(referer) = referer {
            request = request.header(REFERER, referer);
        }
        self.web(request).await
    }

    /// Calls a Web API method with the access token; parses the JSON answer.
    pub(crate) async fn api<T: DeserializeOwned>(
        &self,
        post: bool,
        interface: &str,
        method: &str,
        version: u32,
        params: &[(&str, String)],
    ) -> Result<T> {
        let token = self.access_token().await?;
        let url = format!("{}/{interface}/{method}/v{version}/", self.0.urls.api);
        let mut all = vec![("access_token", token.expose_secret().to_string())];
        all.extend(params.iter().cloned());
        let request = if post { self.0.http.post(url).form(&all) } else { self.0.http.get(url).query(&all) };
        let response = request.send().await?;
        check_eresult(&response)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::SessionExpired);
        }
        json(response).await
    }

    /// Calls a protobuf Web API service method with the access token.
    pub(crate) async fn api_protobuf<Req: Message, Resp: Message + Default>(
        &self,
        post: bool,
        interface: &str,
        method: &str,
        version: u32,
        request: &Req,
    ) -> Result<Resp> {
        let token = self.access_token().await?;
        let url = format!("{}/{interface}/{method}/v{version}/", self.0.urls.api);
        let encoded = base64::engine::general_purpose::STANDARD.encode(request.encode_to_vec());
        let params = [
            ("access_token", token.expose_secret()),
            ("format", "protobuf_raw"),
            ("input_protobuf_encoded", encoded.as_str()),
        ];
        let request = if post { self.0.http.post(url).form(&params) } else { self.0.http.get(url).query(&params) };
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::SessionExpired);
        }
        check_eresult(&response)?;
        if !response.status().is_success() {
            return Err(Error::HttpStatus(response.status().as_u16()));
        }
        Resp::decode(response.bytes().await?).map_err(|e| Error::invalid(format!("{interface}.{method}: {e}")))
    }
}

/// Fails with the `x-eresult` header's result if it isn't OK.
fn check_eresult(response: &Response) -> Result<()> {
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok());
    let Some(result) = header("x-eresult").and_then(|v| v.parse().ok()).map(EResult) else {
        return Ok(());
    };
    if result == EResult::OK {
        return Ok(());
    }
    Err(Error::Steam { result, message: header("x-error_message").map(str::to_string) })
}

/// Parses a JSON answer. An error status becomes the `strError` Steam sent
/// with it, if any.
pub(crate) async fn json<T: DeserializeOwned>(response: Response) -> Result<T> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(steam_error(&body).unwrap_or(Error::HttpStatus(status.as_u16())));
    }
    serde_json::from_str(&body).map_err(|e| {
        if body.trim_start().starts_with('<') {
            Error::invalid(format!("HTML instead of JSON ({e})"))
        } else {
            Error::invalid(format!("{e}: {}", truncate(&body, 200)))
        }
    })
}

/// Steam's `{"strError": "... (15)"}` as an error, the number in brackets as
/// the result.
pub(crate) fn steam_error(body: &str) -> Option<Error> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let message = value["strError"].as_str()?.to_string();
    let result = message
        .rsplit_once('(')
        .and_then(|(_, n)| n.strip_suffix(')'))
        .and_then(|n| n.trim().parse().ok())
        .map_or(EResult::Fail, EResult);
    Some(Error::Steam { result, message: Some(message) })
}

pub(crate) fn truncate(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// Encodes like JavaScript's `encodeURIComponent`, as Steam's own clients do
/// for `steamLoginSecure`.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                encoded.push(byte as char)
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(claims: &str) -> String {
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        format!("{}.{}.sig", b64(r#"{"alg":"EdDSA"}"#), b64(claims))
    }

    #[test]
    fn token_lifetime_from_claims() {
        let token = jwt(r#"{"iss":"steam","exp":1700086400,"iat":1700000000}"#);
        assert_eq!(token_lifetime(&token), Some(Duration::from_secs(86400)));
        assert_eq!(token_lifetime("not a jwt"), None);
    }

    #[test]
    fn renewal_is_an_hour_early_but_not_immediate() {
        let now = Instant::now();
        let day = Token::new(jwt(r#"{"exp":86400,"iat":0}"#), true, now);
        assert_eq!(day.renew_at, Some(now + Duration::from_secs(86400 - 3600)));
        let short = Token::new(jwt(r#"{"exp":60,"iat":0}"#), true, now);
        assert_eq!(short.renew_at, Some(now + MIN_RENEWAL_INTERVAL));
        assert_eq!(Token::new(jwt(r#"{"exp":60,"iat":0}"#), false, now).renew_at, None);
    }

    #[test]
    fn login_cookie_is_uri_encoded() {
        assert_eq!(encode_uri_component("76561198000000001||a.b-c_d"), "76561198000000001%7C%7Ca.b-c_d");
    }

    #[test]
    fn steam_error_reads_the_result() {
        let e = steam_error(r#"{"strError":"There was an error accepting this trade offer. (28)"}"#).unwrap();
        assert_eq!(e.result(), Some(EResult(28)));
        assert_eq!(steam_error(r#"{"strError":"no number"}"#).unwrap().result(), Some(EResult::Fail));
        assert!(steam_error("<html>").is_none());
    }
}
