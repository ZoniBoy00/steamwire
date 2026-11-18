//! Steam Guard mobile authenticator codes from the account's secrets
//! (ported from SteamHelper-rs's `steam-totp`, itself a port of
//! node-steam-totp).

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use sha1::{Digest, Sha1};

use super::Authenticator;
use crate::error::{Error, Result};
use crate::webapi::WebApi;

type HmacSha1 = Hmac<Sha1>;

const CODE_CHARS: &[u8; 26] = b"23456789BCDFGHJKMNPQRTVWXY";

/// A `shared_secret` or `identity_secret` from the mobile authenticator.
#[derive(Clone)]
pub struct Secret(Vec<u8>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// The raw secret.
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self(secret.into())
    }

    /// A base64 secret, as stored in `.maFile`s.
    pub fn from_base64(secret: &str) -> Result<Self> {
        base64::engine::general_purpose::STANDARD
            .decode(secret.trim())
            .map(Self)
            .map_err(|e| Error::InvalidData(format!("secret is not base64: {e}")))
    }

    /// A hex secret.
    pub fn from_hex(secret: &str) -> Result<Self> {
        hex::decode(secret.trim()).map(Self).map_err(|e| Error::InvalidData(format!("secret is not hex: {e}")))
    }

    fn hmac(&self, data: &[u8]) -> [u8; 20] {
        let mut mac = <HmacSha1 as KeyInit>::new_from_slice(&self.0).expect("any key length");
        mac.update(data);
        mac.finalize().into_bytes().into()
    }
}

/// The current Unix time in seconds.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How many seconds Steam's clock is ahead of ours
/// (`ITwoFactorService/QueryTime`). Add it to [`now`] for the code time.
pub async fn server_time_offset(api: &WebApi) -> Result<i64> {
    let response: serde_json::Value = api.post("ITwoFactorService", "QueryTime", 1, &[]).await?;
    let server_time = &response["response"]["server_time"];
    let server_time = server_time
        .as_str()
        .and_then(|t| t.parse::<i64>().ok())
        .or_else(|| server_time.as_i64())
        .ok_or_else(|| Error::InvalidData("QueryTime returned no server_time".into()))?;
    Ok(server_time - now() as i64)
}

/// The 5-character login code for `time` (Unix seconds); codes change
/// every 30 seconds.
pub fn generate_auth_code(shared_secret: &Secret, time: u64) -> String {
    let digest = shared_secret.hmac(&(time / 30).to_be_bytes());
    let start = usize::from(digest[19] & 0x0F);
    let mut full = u32::from_be_bytes(digest[start..start + 4].try_into().expect("4 bytes")) & 0x7FFF_FFFF;
    (0..5)
        .map(|_| {
            let c = CODE_CHARS[(full % 26) as usize];
            full /= 26;
            char::from(c)
        })
        .collect()
}

/// The key for the mobile confirmations web page. `tag` says what it's for:
/// `"conf"` to list confirmations, `"details"`, `"allow"` or `"cancel"`.
pub fn generate_confirmation_key(identity_secret: &Secret, time: u64, tag: &str) -> String {
    let mut data = time.to_be_bytes().to_vec();
    data.extend(tag.bytes().take(32));
    base64::engine::general_purpose::STANDARD.encode(identity_secret.hmac(&data))
}

/// The device ID the mobile app derives from the Steam ID, e.g.
/// `android:cd5f79f7-6eb7-77fb-f3c6-211c848cf6d1`.
pub fn device_id(steam_id: &str) -> String {
    let h = hex::encode(Sha1::digest(steam_id.as_bytes()));
    format!("android:{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Answers device code prompts with codes generated from the
/// `shared_secret`, so a login needs no one at the console.
#[derive(Clone, Debug)]
pub struct SharedSecretAuthenticator {
    secret: Secret,
    /// Seconds to add to the local clock (see [`server_time_offset`]).
    pub time_offset: i64,
}

impl SharedSecretAuthenticator {
    /// Uses the local clock.
    pub fn new(shared_secret: Secret) -> Self {
        Self { secret: shared_secret, time_offset: 0 }
    }

    /// Aligns the clock with Steam's first.
    pub async fn with_server_time(shared_secret: Secret, api: &WebApi) -> Result<Self> {
        Ok(Self { secret: shared_secret, time_offset: server_time_offset(api).await? })
    }
}

impl Authenticator for SharedSecretAuthenticator {
    async fn device_code(&mut self, previous_code_was_incorrect: bool) -> Result<String> {
        let mut time = now().saturating_add_signed(self.time_offset);
        if previous_code_was_incorrect {
            // The code may have rolled over; wait for the next one.
            let wait = 30 - time % 30;
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            time += wait;
        }
        Ok(generate_auth_code(&self.secret, time))
    }

    async fn email_code(&mut self, _email: &str, _previous_code_was_incorrect: bool) -> Result<String> {
        Err(Error::Other("the account wants an email code, but only a shared secret was given".into()))
    }

    /// Prefers the code over confirming in the app.
    async fn accept_device_confirmation(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from steam-totp's tests.
    const TIME: u64 = 1_572_580_000;

    fn secret() -> Secret {
        Secret::from_hex("deadbeefcafe00").unwrap()
    }

    #[test]
    fn auth_code() {
        assert_eq!(generate_auth_code(&secret(), TIME), "6RFHH");
        let b64 = Secret::from_base64(
            &base64::engine::general_purpose::STANDARD.encode(hex::decode("deadbeefcafe00").unwrap()),
        )
        .unwrap();
        assert_eq!(generate_auth_code(&b64, TIME), "6RFHH");
    }

    #[test]
    fn confirmation_key() {
        assert_eq!(generate_confirmation_key(&secret(), TIME, ""), "Y3orSQpLIsycZY/6shH8j/1UwRY=");
        assert_eq!(generate_confirmation_key(&secret(), TIME, "details"), "uofPzqUhpWkuPH4ZWuRSWejdfAw=");
    }

    #[test]
    fn device_ids() {
        assert_eq!(device_id("myWonderfulSteamId"), "android:cd5f79f7-6eb7-77fb-f3c6-211c848cf6d1");
    }
}
