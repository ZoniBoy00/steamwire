//! Adding and removing the Steam Guard mobile authenticator, over the
//! client connection (`TwoFactor.*` service methods), as the Steam mobile
//! app does.
//!
//! The flow and the `.maFile` follow SteamHelper-rs's `steam-mobile` (MIT,
//! Copyright (c) 2020 Martin Mariano) and Steam Desktop Authenticator.
//!
//! **Adding an authenticator replaces the one the account has, and removing
//! it can lock the account out of trading for days. Save the [`MobileAuth`]
//! (it holds the revocation code) before finalizing: without it, the account
//! can only be recovered through Steam Support.**

use std::path::Path;

use crate::handlers::auth::guard::{self, Secret};
use crate::protos::steam::{
    CTwoFactorAddAuthenticatorRequest, CTwoFactorAddAuthenticatorResponse, CTwoFactorFinalizeAddAuthenticatorRequest,
    CTwoFactorRemoveAuthenticatorRequest, CTwoFactorRemoveAuthenticatorResponse, CTwoFactorStatusRequest,
    CTwoFactorStatusResponse,
};
use crate::protos::ServiceTransport;
use crate::{EResult, SteamId};
use serde::{Deserialize, Serialize};

use crate::community::error::{Error, Result};

/// How many codes [`finalize`] sends before it gives up.
const FINALIZE_ATTEMPTS: usize = 30;

/// `FinalizeAddAuthenticator`'s response with `want_more`, which Steam still
/// sends but the published protos no longer list.
#[derive(Clone, PartialEq, prost::Message)]
struct FinalizeResponse {
    #[prost(bool, optional, tag = "1")]
    success: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    want_more: Option<bool>,
    #[prost(uint64, optional, tag = "3")]
    server_time: Option<u64>,
    #[prost(int32, optional, tag = "4")]
    status: Option<i32>,
}

/// The status Steam sends for a wrong activation code.
const STATUS_ACTIVATION_CODE_MISMATCH: i32 = 89;
/// The status Steam sends for a wrong authenticator code.
const STATUS_CODE_MISMATCH: i32 = 88;

/// The authenticator's secrets, in Steam Desktop Authenticator's `.maFile`
/// format. Its `Debug` hides the secrets; the file grants full control of
/// the account, so keep it private.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MobileAuth {
    /// Makes the login codes (base64).
    pub shared_secret: String,
    /// The authenticator's serial number.
    #[serde(default)]
    pub serial_number: String,
    /// Removes the authenticator ("R12345").
    pub revocation_code: String,
    /// The `otpauth://` URI.
    #[serde(default)]
    pub uri: String,
    /// Steam's time when it was made.
    #[serde(default)]
    pub server_time: u64,
    /// The account's name.
    #[serde(default)]
    pub account_name: String,
    /// The token GID.
    #[serde(default)]
    pub token_gid: String,
    /// Makes the confirmation keys (base64).
    pub identity_secret: String,
    /// The third secret (base64).
    #[serde(default)]
    pub secret_1: String,
    /// The status Steam sent when it was made.
    #[serde(default)]
    pub status: i32,
    /// The device ID it was registered with.
    #[serde(default)]
    pub device_id: String,
    /// Finalizing succeeded.
    #[serde(default)]
    pub fully_enrolled: bool,
    /// The account (`{"SteamID": …}`), as Steam Desktop Authenticator keeps it.
    #[serde(rename = "Session", default, skip_serializing_if = "Option::is_none")]
    pub session: Option<MaFileSession>,
}

/// The session part of an `.maFile`: only the Steam ID is kept.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MaFileSession {
    /// The account's Steam ID.
    #[serde(rename = "SteamID")]
    pub steam_id: u64,
}

impl std::fmt::Debug for MobileAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobileAuth")
            .field("account_name", &self.account_name)
            .field("device_id", &self.device_id)
            .field("fully_enrolled", &self.fully_enrolled)
            .finish_non_exhaustive()
    }
}

impl MobileAuth {
    /// Reads an `.maFile` (unencrypted).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|e| Error::InvalidInput(format!("not an maFile: {e}")))
    }

    /// Writes it as an `.maFile`, replacing the file only once the new one
    /// is complete. On Unix the file is readable by its owner only (0600):
    /// it holds the secrets.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        use std::io::Write as _;

        let path = path.as_ref();
        let partial = path.with_extension("maFile.partial");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial)?;
        // a partial file left from before keeps its mode: set it again before writing
        #[cfg(unix)]
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(&serde_json::to_vec_pretty(self).expect("serializable"))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&partial, path)?;
        Ok(())
    }

    /// The secret for login codes.
    pub fn shared_secret(&self) -> Result<Secret> {
        Ok(Secret::from_base64(&self.shared_secret)?)
    }

    /// The secret for confirmation keys.
    pub fn identity_secret(&self) -> Result<Secret> {
        Ok(Secret::from_base64(&self.identity_secret)?)
    }

    /// The account, if the file says.
    pub fn steam_id(&self) -> Option<SteamId> {
        self.session.as_ref().map(|s| SteamId::from_u64(s.steam_id))
    }

    /// The login code for now, with Steam's clock `time_offset` seconds
    /// ahead of ours.
    pub fn login_code(&self, time_offset: i64) -> Result<String> {
        Ok(guard::generate_auth_code(&self.shared_secret()?, guard::now().saturating_add_signed(time_offset)))
    }
}

/// How Steam sent the activation code [`finalize`] needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodeDelivery {
    /// By SMS to the account's phone.
    Sms,
    /// By email.
    Email,
    /// Something else (Steam's `confirm_type`).
    Other(i32),
}

impl From<i32> for CodeDelivery {
    fn from(confirm_type: i32) -> Self {
        match confirm_type {
            1 => Self::Sms,
            3 => Self::Email,
            other => Self::Other(other),
        }
    }
}

/// An authenticator Steam made but that isn't active until [`finalize`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PendingAuthenticator {
    /// Its secrets. **Save them before finalizing.**
    pub mobile_auth: MobileAuth,
    /// Where the activation code went.
    pub code_delivery: CodeDelivery,
    /// The end of the phone number the SMS went to.
    pub phone_number_hint: Option<String>,
}

/// What the account's mobile authenticator is
/// (`TwoFactor.QueryStatus`).
pub async fn status<T>(transport: &T, steam_id: SteamId) -> Result<CTwoFactorStatusResponse>
where
    T: ServiceTransport<Error = crate::Error> + ?Sized,
{
    let request = CTwoFactorStatusRequest { steamid: Some(steam_id.as_u64()), ..Default::default() };
    Ok(transport.call("TwoFactor.QueryStatus#1", request).await?)
}

/// Starts adding a mobile authenticator to the account the client is logged
/// on as (`TwoFactor.AddAuthenticator`). Steam sends an activation code by
/// SMS or email; pass it to [`finalize`].
///
/// Fails with `DuplicateRequest` if the account has an authenticator, and
/// `NoVerifiedPhone` if it needs a phone number first.
pub async fn add<T>(transport: &T, steam_id: SteamId) -> Result<PendingAuthenticator>
where
    T: ServiceTransport<Error = crate::Error> + ?Sized,
{
    let device_id = guard::device_id(&steam_id.as_u64().to_string());
    let request = CTwoFactorAddAuthenticatorRequest {
        steamid: Some(steam_id.as_u64()),
        authenticator_type: Some(1),
        device_identifier: Some(device_id.clone()),
        // Version 2 allows device confirmation logins.
        version: Some(2),
        ..Default::default()
    };
    let response: CTwoFactorAddAuthenticatorResponse = transport.call("TwoFactor.AddAuthenticator#1", request).await?;
    let status = response.status.unwrap_or(1);
    if status != 1 {
        return Err(Error::steam(EResult(status)));
    }
    let b64 = |bytes: Option<Vec<u8>>| {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes.unwrap_or_default())
    };
    if response.shared_secret.as_ref().is_none_or(Vec::is_empty) || response.revocation_code.is_none() {
        return Err(Error::invalid("AddAuthenticator returned no secrets"));
    }
    let mobile_auth = MobileAuth {
        shared_secret: b64(response.shared_secret),
        serial_number: response.serial_number.unwrap_or_default().to_string(),
        revocation_code: response.revocation_code.unwrap_or_default(),
        uri: response.uri.unwrap_or_default(),
        server_time: response.server_time.unwrap_or_default(),
        account_name: response.account_name.unwrap_or_default(),
        token_gid: response.token_gid.unwrap_or_default(),
        identity_secret: b64(response.identity_secret),
        secret_1: b64(response.secret_1),
        status,
        device_id,
        fully_enrolled: false,
        session: Some(MaFileSession { steam_id: steam_id.as_u64() }),
    };
    Ok(PendingAuthenticator {
        mobile_auth,
        code_delivery: CodeDelivery::from(response.confirm_type.unwrap_or_default()),
        phone_number_hint: response.phone_number_hint.filter(|h| !h.is_empty()),
    })
}

/// Activates the authenticator [`add`] made, with the activation code Steam
/// sent (`TwoFactor.FinalizeAddAuthenticator`). Sends codes until Steam has
/// seen enough; `time_offset` is how far Steam's clock is ahead of ours.
/// Marks `mobile_auth` fully enrolled on success: save it again.
pub async fn finalize<T>(
    transport: &T,
    mobile_auth: &mut MobileAuth,
    activation_code: &str,
    time_offset: i64,
) -> Result<()>
where
    T: ServiceTransport<Error = crate::Error> + ?Sized,
{
    let steam_id = mobile_auth.steam_id().ok_or_else(|| Error::InvalidInput("the maFile has no Steam ID".into()))?;
    let secret = mobile_auth.shared_secret()?;
    let mut time = guard::now().saturating_add_signed(time_offset);
    for _ in 0..FINALIZE_ATTEMPTS {
        let request = CTwoFactorFinalizeAddAuthenticatorRequest {
            steamid: Some(steam_id.as_u64()),
            authenticator_code: Some(guard::generate_auth_code(&secret, time)),
            authenticator_time: Some(time),
            activation_code: Some(activation_code.to_string()),
            validate_sms_code: Some(true),
            ..Default::default()
        };
        let response: Result<FinalizeResponse, _> =
            transport.call("TwoFactor.FinalizeAddAuthenticator#1", request).await;
        let response = match response {
            Ok(response) => response,
            Err(e) => match e.result() {
                Some(EResult::TwoFactorActivationCodeMismatch) => return Err(Error::ActivationCodeMismatch),
                Some(EResult::TwoFactorCodeMismatch) => {
                    FinalizeResponse { status: Some(STATUS_CODE_MISMATCH), ..Default::default() }
                }
                _ => return Err(e.into()),
            },
        };
        match response.status {
            Some(STATUS_ACTIVATION_CODE_MISMATCH) => return Err(Error::ActivationCodeMismatch),
            Some(STATUS_CODE_MISMATCH) => {}
            _ if response.success == Some(true) && response.want_more != Some(true) => {
                mobile_auth.fully_enrolled = true;
                return Ok(());
            }
            _ if response.success == Some(true) => {}
            Some(status) => return Err(Error::steam(EResult(status))),
            None => return Err(Error::invalid("FinalizeAddAuthenticator failed without a status")),
        }
        // The code of the next 30-second window, as node-steam-user does.
        time += 30;
    }
    Err(Error::CodesRejected)
}

/// What the account keeps after [`remove`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardScheme {
    /// Steam Guard codes by email.
    Email,
    /// No Steam Guard at all.
    None,
}

/// Removes the account's mobile authenticator with its revocation code
/// (`TwoFactor.RemoveAuthenticator`). A wrong code fails with
/// [`Error::IncorrectRevocationCode`]; too many lock the account.
pub async fn remove<T>(transport: &T, revocation_code: &str, scheme: GuardScheme) -> Result<()>
where
    T: ServiceTransport<Error = crate::Error> + ?Sized,
{
    let request = CTwoFactorRemoveAuthenticatorRequest {
        revocation_code: Some(revocation_code.to_string()),
        revocation_reason: Some(1),
        steamguard_scheme: Some(match scheme {
            GuardScheme::Email => 1,
            GuardScheme::None => 2,
        }),
        remove_all_steamguard_cookies: None,
    };
    let response: CTwoFactorRemoveAuthenticatorResponse =
        transport.call("TwoFactor.RemoveAuthenticator#1", request).await?;
    if response.success == Some(true) {
        Ok(())
    } else {
        Err(Error::IncorrectRevocationCode { attempts_remaining: response.revocation_attempts_remaining.unwrap_or(0) })
    }
}
