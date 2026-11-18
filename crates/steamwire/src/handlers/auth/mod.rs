//! The `Authentication` service flows: QR code and username/password with
//! Steam Guard (SteamKit2's `SteamAuthentication`).
//!
//! Both flows end with a refresh token, which is then passed to
//! [`SteamClient::log_on`] as [`LogOnDetails::access_token`](crate::handlers::user::LogOnDetails::access_token).

mod authenticator;
pub mod guard;

use std::time::Duration;

use base64::Engine as _;
use steamwire_protos::steam::{
    CAuthenticationAccessTokenGenerateForAppRequest, CAuthenticationAccessTokenGenerateForAppResponse,
    CAuthenticationAllowedConfirmation, CAuthenticationBeginAuthSessionViaCredentialsRequest,
    CAuthenticationBeginAuthSessionViaCredentialsResponse, CAuthenticationBeginAuthSessionViaQrRequest,
    CAuthenticationBeginAuthSessionViaQrResponse, CAuthenticationDeviceDetails,
    CAuthenticationGetPasswordRsaPublicKeyRequest, CAuthenticationGetPasswordRsaPublicKeyResponse,
    CAuthenticationPollAuthSessionStatusRequest, CAuthenticationPollAuthSessionStatusResponse,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest,
    CAuthenticationUpdateAuthSessionWithSteamGuardCodeResponse, EAuthSessionGuardType, EAuthTokenPlatformType,
    ESessionPersistence,
};

use crate::client::SteamClient;
use crate::enums::{EOSType, EResult};
use crate::error::{Error, Result};
use crate::handlers::user::default_os_type;
use crate::types::SteamId;
use crate::util::crypto;

pub use authenticator::{Authenticator, UserConsoleAuthenticator};
pub use guard::SharedSecretAuthenticator;

/// Device details sent when starting an auth session.
#[derive(Clone, Debug)]
pub struct AuthSessionDetails {
    /// The name shown in the account's device list and in the mobile app.
    pub device_friendly_name: String,
    /// The platform the tokens are for. Use `SteamClient` to log on to a CM.
    pub platform_type: EAuthTokenPlatformType,
    /// The OS reported to Steam.
    pub os_type: EOSType,
    /// The website ID; `"Client"` for the Steam client.
    pub website_id: String,
    /// Whether the refresh token should be long-lived.
    pub persistent: bool,
    /// Guard data from a previous session ([`AuthTokens::new_guard_data`]),
    /// which skips the email code on a known device.
    pub guard_data: Option<String>,
}

impl Default for AuthSessionDetails {
    fn default() -> Self {
        Self {
            device_friendly_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .map(|h| format!("{h} (steamkit-rs)"))
                .unwrap_or_else(|_| "steamkit-rs".into()),
            platform_type: EAuthTokenPlatformType::KEAuthTokenPlatformTypeSteamClient,
            os_type: default_os_type(),
            website_id: "Client".into(),
            persistent: true,
            guard_data: None,
        }
    }
}

impl AuthSessionDetails {
    fn device_details(&self) -> CAuthenticationDeviceDetails {
        CAuthenticationDeviceDetails {
            device_friendly_name: Some(self.device_friendly_name.clone()),
            platform_type: Some(self.platform_type as i32),
            os_type: Some(self.os_type.0),
            ..Default::default()
        }
    }
}

/// The tokens issued when an auth session completes.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AuthTokens {
    /// The account name.
    pub account_name: String,
    /// The refresh token: pass it to `log_on` as the access token.
    pub refresh_token: String,
    /// A short-lived access token (for the Web API).
    pub access_token: String,
    /// Guard data to remember this device; store it for the next session.
    pub new_guard_data: Option<String>,
}

/// The kind of Steam Guard code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardCodeType {
    /// The code emailed to the account.
    Email,
    /// The code from the mobile authenticator.
    Device,
}

#[derive(Debug)]
struct Poller {
    client: SteamClient,
    client_id: u64,
    request_id: Vec<u8>,
    interval: Duration,
    allowed_confirmations: Vec<CAuthenticationAllowedConfirmation>,
}

impl Poller {
    async fn poll(&mut self) -> Result<(Option<AuthTokens>, Option<String>)> {
        let response: CAuthenticationPollAuthSessionStatusResponse = self
            .client
            .call_service(
                "Authentication.PollAuthSessionStatus#1",
                CAuthenticationPollAuthSessionStatusRequest {
                    client_id: Some(self.client_id),
                    request_id: Some(self.request_id.clone()),
                    ..Default::default()
                },
            )
            .await?;

        if let Some(id) = response.new_client_id {
            self.client_id = id;
        }
        let new_url = response.new_challenge_url.clone().filter(|u| !u.is_empty());
        let tokens = response.refresh_token.filter(|t| !t.is_empty()).map(|refresh_token| AuthTokens {
            account_name: response.account_name.unwrap_or_default(),
            refresh_token,
            access_token: response.access_token.unwrap_or_default(),
            new_guard_data: response.new_guard_data,
        });
        Ok((tokens, new_url))
    }
}

/// An auth session started with [`SteamClient::begin_auth_session_via_qr`].
#[derive(Debug)]
pub struct QrAuthSession {
    poller: Poller,
    challenge_url: String,
}

impl QrAuthSession {
    /// The client ID of the session; it may change while polling.
    pub fn client_id(&self) -> u64 {
        self.poller.client_id
    }

    /// The request ID of the session.
    pub fn request_id(&self) -> &[u8] {
        &self.poller.request_id
    }

    /// The URL to show as a QR code. It changes over time; see
    /// [`QrAuthSession::poll`].
    pub fn challenge_url(&self) -> &str {
        &self.challenge_url
    }

    /// The polling interval Steam asked for.
    pub fn interval(&self) -> Duration {
        self.poller.interval
    }

    /// Polls once. Returns the tokens once the code was scanned and
    /// approved. When the challenge URL changes, [`challenge_url`](Self::challenge_url)
    /// is updated and `on_new_url` is true in the returned tuple.
    pub async fn poll(&mut self) -> Result<(Option<AuthTokens>, bool)> {
        let (tokens, new_url) = self.poller.poll().await?;
        let changed = match new_url {
            Some(url) if url != self.challenge_url => {
                self.challenge_url = url;
                true
            }
            _ => false,
        };
        Ok((tokens, changed))
    }

    /// Polls until the session completes, calling `on_new_url` whenever the
    /// QR code has to be redrawn.
    pub async fn wait(&mut self, mut on_new_url: impl FnMut(&str)) -> Result<AuthTokens> {
        loop {
            tokio::time::sleep(self.poller.interval).await;
            let (tokens, changed) = self.poll().await?;
            if changed {
                on_new_url(&self.challenge_url);
            }
            if let Some(tokens) = tokens {
                return Ok(tokens);
            }
        }
    }
}

/// An auth session started with [`SteamClient::begin_auth_session_via_credentials`].
#[derive(Debug)]
pub struct CredentialsAuthSession {
    poller: Poller,
    steam_id: SteamId,
}

impl CredentialsAuthSession {
    /// The client ID of the session; it may change while polling.
    pub fn client_id(&self) -> u64 {
        self.poller.client_id
    }

    /// The request ID of the session.
    pub fn request_id(&self) -> &[u8] {
        &self.poller.request_id
    }

    /// The Steam ID of the account.
    pub fn steam_id(&self) -> SteamId {
        self.steam_id
    }

    /// The confirmations Steam allows, most preferred first.
    pub fn allowed_confirmations(&self) -> Vec<EAuthSessionGuardType> {
        self.poller.allowed_confirmations.iter().map(CAuthenticationAllowedConfirmation::confirmation_type).collect()
    }

    /// Which code Steam wants, if any. `None` means no code is needed, or the
    /// login is confirmed in the mobile app / by email link (just call
    /// [`wait`](Self::wait)).
    pub fn required_code(&self) -> Option<GuardCodeType> {
        let preferred = self.allowed_confirmations().into_iter().next()?;
        match preferred {
            EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode => Some(GuardCodeType::Email),
            EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode => Some(GuardCodeType::Device),
            _ => None,
        }
    }

    /// Submits a Steam Guard code. A wrong code fails with
    /// `EResult::InvalidLoginAuthCode` (email) or
    /// `EResult::TwoFactorCodeMismatch` (device).
    pub async fn submit_code(&self, code: &str, code_type: GuardCodeType) -> Result<()> {
        let code_type = match code_type {
            GuardCodeType::Email => EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode,
            GuardCodeType::Device => EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode,
        };
        let response: Result<CAuthenticationUpdateAuthSessionWithSteamGuardCodeResponse> = self
            .poller
            .client
            .call_service(
                "Authentication.UpdateAuthSessionWithSteamGuardCode#1",
                CAuthenticationUpdateAuthSessionWithSteamGuardCodeRequest {
                    client_id: Some(self.poller.client_id),
                    steamid: Some(self.steam_id.as_u64()),
                    code: Some(code.to_string()),
                    code_type: Some(code_type as i32),
                },
            )
            .await;
        match response {
            Ok(_) => Ok(()),
            // As in SteamKit2: Steam answers DuplicateRequest when the login
            // was already approved in the mobile app; the next poll succeeds.
            Err(e) if e.result() == Some(EResult::DuplicateRequest) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Polls once; the tokens once the session completes.
    pub async fn poll(&mut self) -> Result<Option<AuthTokens>> {
        Ok(self.poller.poll().await?.0)
    }

    /// Polls until the session completes.
    pub async fn wait(&mut self) -> Result<AuthTokens> {
        loop {
            if let Some(tokens) = self.poll().await? {
                return Ok(tokens);
            }
            tokio::time::sleep(self.poller.interval).await;
        }
    }
}

fn sorted(mut confirmations: Vec<CAuthenticationAllowedConfirmation>) -> Vec<CAuthenticationAllowedConfirmation> {
    authenticator::sort_confirmations(&mut confirmations);
    confirmations
}

fn interval(seconds: Option<f32>) -> Duration {
    Duration::from_secs_f32(seconds.unwrap_or(5.0).clamp(0.5, 60.0))
}

impl SteamClient {
    /// Starts a QR code login. Show [`QrAuthSession::challenge_url`] as a QR
    /// code, scan it with the Steam mobile app, then [`QrAuthSession::wait`].
    pub async fn begin_auth_session_via_qr(&self, details: &AuthSessionDetails) -> Result<QrAuthSession> {
        let response: CAuthenticationBeginAuthSessionViaQrResponse = self
            .call_service(
                "Authentication.BeginAuthSessionViaQR#1",
                CAuthenticationBeginAuthSessionViaQrRequest {
                    device_friendly_name: Some(details.device_friendly_name.clone()),
                    platform_type: Some(details.platform_type as i32),
                    device_details: Some(details.device_details()),
                    website_id: Some(details.website_id.clone()),
                },
            )
            .await?;

        Ok(QrAuthSession {
            challenge_url: response.challenge_url.clone().unwrap_or_default(),
            poller: Poller {
                client: self.clone(),
                client_id: response.client_id(),
                request_id: response.request_id.clone().unwrap_or_default(),
                interval: interval(response.interval),
                allowed_confirmations: sorted(response.allowed_confirmations),
            },
        })
    }

    /// Starts a username/password login. The password is RSA encrypted with
    /// the key from `Authentication.GetPasswordRSAPublicKey`.
    pub async fn begin_auth_session_via_credentials(
        &self,
        username: &str,
        password: &str,
        details: &AuthSessionDetails,
    ) -> Result<CredentialsAuthSession> {
        if username.is_empty() || password.is_empty() {
            return Err(Error::Other("username and password are required".into()));
        }
        let key: CAuthenticationGetPasswordRsaPublicKeyResponse = self
            .call_service(
                "Authentication.GetPasswordRSAPublicKey#1",
                CAuthenticationGetPasswordRsaPublicKeyRequest { account_name: Some(username.into()) },
            )
            .await?;
        let encrypted = crypto::rsa_encrypt_pkcs1_hex(key.publickey_mod(), key.publickey_exp(), password.as_bytes())?;

        let persistence = if details.persistent {
            ESessionPersistence::KESessionPersistencePersistent
        } else {
            ESessionPersistence::KESessionPersistenceEphemeral
        };
        let response: CAuthenticationBeginAuthSessionViaCredentialsResponse = self
            .call_service(
                "Authentication.BeginAuthSessionViaCredentials#1",
                CAuthenticationBeginAuthSessionViaCredentialsRequest {
                    account_name: Some(username.into()),
                    encrypted_password: Some(base64::engine::general_purpose::STANDARD.encode(encrypted)),
                    encryption_timestamp: key.timestamp,
                    persistence: Some(persistence as i32),
                    website_id: Some(details.website_id.clone()),
                    guard_data: details.guard_data.clone(),
                    device_details: Some(details.device_details()),
                    ..Default::default()
                },
            )
            .await?;

        Ok(CredentialsAuthSession {
            steam_id: SteamId::from_u64(response.steamid()),
            poller: Poller {
                client: self.clone(),
                client_id: response.client_id(),
                request_id: response.request_id.clone().unwrap_or_default(),
                interval: interval(response.interval),
                allowed_confirmations: sorted(response.allowed_confirmations),
            },
        })
    }

    /// Exchanges a refresh token for a new access token
    /// (`Authentication.GenerateAccessTokenForApp`).
    pub async fn generate_access_token(
        &self,
        steam_id: SteamId,
        refresh_token: &str,
        allow_renewal: bool,
    ) -> Result<CAuthenticationAccessTokenGenerateForAppResponse> {
        self.call_service(
            "Authentication.GenerateAccessTokenForApp#1",
            CAuthenticationAccessTokenGenerateForAppRequest {
                refresh_token: Some(refresh_token.into()),
                steamid: Some(steam_id.as_u64()),
                renewal_type: Some(if allow_renewal { 1 } else { 0 }),
            },
        )
        .await
    }
}
