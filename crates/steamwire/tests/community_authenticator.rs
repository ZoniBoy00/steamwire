//! Adding and removing the authenticator against a stand-in for the client
//! connection; never against a real account.

mod common;

use common::{FakeTransport, ME};
use steamwire::community::authenticator::{self, CodeDelivery, GuardScheme};
use steamwire::community::{Error, MobileAuth};
use steamwire::handlers::auth::guard::{self, Secret};
use steamwire::protos::steam::{
    CTwoFactorAddAuthenticatorRequest, CTwoFactorAddAuthenticatorResponse, CTwoFactorFinalizeAddAuthenticatorRequest,
    CTwoFactorRemoveAuthenticatorRequest, CTwoFactorRemoveAuthenticatorResponse,
};
use steamwire::EResult;

/// `FinalizeAddAuthenticator`'s answer with `want_more` (field 2).
#[derive(Clone, PartialEq, prost::Message)]
struct Finalized {
    #[prost(bool, optional, tag = "1")]
    success: Option<bool>,
    #[prost(bool, optional, tag = "2")]
    want_more: Option<bool>,
    #[prost(uint64, optional, tag = "3")]
    server_time: Option<u64>,
    #[prost(int32, optional, tag = "4")]
    status: Option<i32>,
}

fn added() -> CTwoFactorAddAuthenticatorResponse {
    CTwoFactorAddAuthenticatorResponse {
        shared_secret: Some(b"shared secret 20 by".to_vec()),
        serial_number: Some(123),
        revocation_code: Some("R12345".into()),
        uri: Some("otpauth://totp/Steam:someone?secret=X&issuer=Steam".into()),
        server_time: Some(1_700_000_000),
        account_name: Some("someone".into()),
        token_gid: Some("gid".into()),
        identity_secret: Some(b"identity secret".to_vec()),
        secret_1: Some(b"secret one".to_vec()),
        status: Some(1),
        phone_number_hint: Some("12".into()),
        confirm_type: Some(1),
    }
}

#[tokio::test]
async fn adding_and_finalizing() {
    let steam = FakeTransport::default();
    steam.answer(EResult::OK, added());
    let pending = authenticator::add(&steam, ME).await.unwrap();
    assert_eq!(pending.code_delivery, CodeDelivery::Sms);
    assert_eq!(pending.phone_number_hint.as_deref(), Some("12"));
    let mut auth = pending.mobile_auth;
    assert_eq!(auth.revocation_code, "R12345");
    assert_eq!(auth.device_id, guard::device_id("76561198000000001"));
    assert_eq!(auth.steam_id(), Some(ME));
    assert!(!auth.fully_enrolled);
    assert!(!format!("{auth:?}").contains("R12345"), "Debug hides the secrets");

    let request: CTwoFactorAddAuthenticatorRequest = steam.call_bodies("TwoFactor.AddAuthenticator#1").remove(0);
    assert_eq!(request.steamid, Some(ME.as_u64()));
    assert_eq!(request.authenticator_type, Some(1));
    assert_eq!(request.device_identifier.as_deref(), Some(auth.device_id.as_str()));

    // Steam wants a second code, then is done.
    steam.answer(EResult::OK, Finalized { success: Some(true), want_more: Some(true), ..Default::default() });
    steam.answer(EResult::OK, Finalized { success: Some(true), status: Some(2), ..Default::default() });
    authenticator::finalize(&steam, &mut auth, "ABCDE", 0).await.unwrap();
    assert!(auth.fully_enrolled);
    let sent: Vec<CTwoFactorFinalizeAddAuthenticatorRequest> =
        steam.call_bodies("TwoFactor.FinalizeAddAuthenticator#1");
    assert_eq!(sent.len(), 2);
    let secret = Secret::new(b"shared secret 20 by".to_vec());
    for request in &sent {
        assert_eq!(request.activation_code.as_deref(), Some("ABCDE"));
        assert_eq!(request.validate_sms_code, Some(true));
        let time = request.authenticator_time.unwrap();
        assert_eq!(request.authenticator_code, Some(guard::generate_auth_code(&secret, time)));
    }
    assert_eq!(sent[1].authenticator_time.unwrap(), sent[0].authenticator_time.unwrap() + 30);
}

#[tokio::test]
async fn adding_fails_with_an_authenticator_present() {
    let steam = FakeTransport::default();
    steam.answer(EResult::DuplicateRequest, CTwoFactorAddAuthenticatorResponse::default());
    let error = authenticator::add(&steam, ME).await.unwrap_err();
    assert_eq!(error.result(), Some(EResult::DuplicateRequest));

    steam.answer(EResult::OK, CTwoFactorAddAuthenticatorResponse { status: Some(29), ..Default::default() });
    assert_eq!(authenticator::add(&steam, ME).await.unwrap_err().result(), Some(EResult(29)));
}

#[tokio::test]
async fn a_wrong_activation_code_stops_finalizing() {
    let steam = FakeTransport::default();
    steam.answer(EResult::OK, added());
    let mut auth = authenticator::add(&steam, ME).await.unwrap().mobile_auth;
    steam.answer(EResult::OK, Finalized { success: Some(false), status: Some(89), ..Default::default() });
    let error = authenticator::finalize(&steam, &mut auth, "WRONG", 0).await.unwrap_err();
    assert!(matches!(error, Error::ActivationCodeMismatch));
    steam.answer(EResult::TwoFactorActivationCodeMismatch, Finalized::default());
    let error = authenticator::finalize(&steam, &mut auth, "WRONG", 0).await.unwrap_err();
    assert!(matches!(error, Error::ActivationCodeMismatch));

    // Codes that keep being wrong end after 30 tries.
    for _ in 0..30 {
        steam.answer(EResult::OK, Finalized { success: Some(false), status: Some(88), ..Default::default() });
    }
    let error = authenticator::finalize(&steam, &mut auth, "ABCDE", 0).await.unwrap_err();
    assert!(matches!(error, Error::CodesRejected));
    assert!(!auth.fully_enrolled);
}

#[tokio::test]
async fn removing() {
    let steam = FakeTransport::default();
    steam.answer(EResult::OK, CTwoFactorRemoveAuthenticatorResponse { success: Some(true), ..Default::default() });
    authenticator::remove(&steam, "R12345", GuardScheme::Email).await.unwrap();
    let request: CTwoFactorRemoveAuthenticatorRequest = steam.call_bodies("TwoFactor.RemoveAuthenticator#1").remove(0);
    assert_eq!(request.revocation_code.as_deref(), Some("R12345"));
    assert_eq!(request.steamguard_scheme, Some(1));

    steam.answer(
        EResult::OK,
        CTwoFactorRemoveAuthenticatorResponse {
            success: Some(false),
            revocation_attempts_remaining: Some(4),
            ..Default::default()
        },
    );
    let error = authenticator::remove(&steam, "R00000", GuardScheme::None).await.unwrap_err();
    assert!(matches!(error, Error::IncorrectRevocationCode { attempts_remaining: 4 }));
}

#[test]
fn mafiles_round_trip() {
    // The format Steam Desktop Authenticator writes.
    let text = r#"{"shared_secret":"c2hhcmVkIHNlY3JldCAyMCBieQ==","serial_number":"123","revocation_code":"R12345",
        "uri":"otpauth://totp/Steam:someone","server_time":1700000000,"account_name":"someone","token_gid":"gid",
        "identity_secret":"aWRlbnRpdHkgc2VjcmV0","secret_1":"c2VjcmV0IG9uZQ==","status":1,
        "device_id":"android:x","fully_enrolled":true,"Session":{"SessionID":"s","SteamLogin":"l","SteamID":76561198000000001}}"#;
    let auth: MobileAuth = serde_json::from_str(text).unwrap();
    assert_eq!(auth.steam_id(), Some(ME));
    assert_eq!(auth.login_code(0).unwrap().len(), 5);

    let dir = std::env::temp_dir().join(format!("steamwire-mafile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("76561198000000001.maFile");
    // a partial file left from an earlier attempt, readable by others
    std::fs::write(path.with_extension("maFile.partial"), "old").unwrap();
    auth.save(&path).unwrap();
    assert_eq!(MobileAuth::load(&path).unwrap(), auth);
    // the secrets are the owner's only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
