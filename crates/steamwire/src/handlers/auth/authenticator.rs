//! Answering Steam Guard prompts while an auth session completes
//! (SteamKit2's `IAuthenticator`, `UserConsoleAuthenticator` and
//! `AuthSession.PollingWaitForResultAsync`).

use std::future::Future;
use std::io::{BufRead, Write};

use steamwire_protos::steam::{CAuthenticationAllowedConfirmation, EAuthSessionGuardType};

use super::{AuthTokens, CredentialsAuthSession, GuardCodeType};
use crate::enums::EResult;
use crate::error::{Error, Result};

/// Provides Steam Guard codes when a login asks for them.
pub trait Authenticator: Send {
    /// A code from the mobile authenticator.
    fn device_code(&mut self, previous_code_was_incorrect: bool) -> impl Future<Output = Result<String>> + Send;

    /// A code emailed to `email` (Steam shows only part of the address).
    fn email_code(
        &mut self,
        email: &str,
        previous_code_was_incorrect: bool,
    ) -> impl Future<Output = Result<String>> + Send;

    /// Whether to wait for the login to be confirmed in the mobile app
    /// (`true`), or to fall back to the next allowed method, usually a code.
    fn accept_device_confirmation(&mut self) -> impl Future<Output = bool> + Send {
        async { true }
    }
}

/// Asks for codes on the console.
#[derive(Clone, Copy, Debug, Default)]
pub struct UserConsoleAuthenticator;

async fn prompt(message: String) -> Result<String> {
    tokio::task::spawn_blocking(move || loop {
        eprint!("{message}");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            return Err(Error::Other("no code entered (end of input)".into()));
        }
        let code = line.trim();
        if !code.is_empty() {
            return Ok(code.to_owned());
        }
    })
    .await
    .map_err(|e| Error::Other(format!("console prompt failed: {e}")))?
}

impl Authenticator for UserConsoleAuthenticator {
    async fn device_code(&mut self, previous_code_was_incorrect: bool) -> Result<String> {
        if previous_code_was_incorrect {
            eprintln!("The previous 2-factor auth code you have provided is incorrect.");
        }
        prompt("STEAM GUARD! Please enter your 2-factor auth code from your authenticator app: ".into()).await
    }

    async fn email_code(&mut self, email: &str, previous_code_was_incorrect: bool) -> Result<String> {
        if previous_code_was_incorrect {
            eprintln!("The previous 2-factor auth code you have provided is incorrect.");
        }
        prompt(format!("STEAM GUARD! Please enter the auth code sent to the email at {email}: ")).await
    }

    async fn accept_device_confirmation(&mut self) -> bool {
        eprintln!("STEAM GUARD! Use the Steam Mobile App to confirm your sign in...");
        true
    }
}

/// The confirmations in the order they're handled in (SteamKit2's
/// `SortConfirmations`); unknown types last.
pub(super) fn sort_confirmations(confirmations: &mut [CAuthenticationAllowedConfirmation]) {
    use EAuthSessionGuardType::*;
    const ORDER: [EAuthSessionGuardType; 7] = [
        KEAuthSessionGuardTypeNone,
        KEAuthSessionGuardTypeDeviceConfirmation,
        KEAuthSessionGuardTypeDeviceCode,
        KEAuthSessionGuardTypeEmailCode,
        KEAuthSessionGuardTypeEmailConfirmation,
        KEAuthSessionGuardTypeMachineToken,
        KEAuthSessionGuardTypeUnknown,
    ];
    confirmations.sort_by_key(|c| ORDER.iter().position(|&t| t == c.confirmation_type()).unwrap_or(usize::MAX));
}

impl CredentialsAuthSession {
    /// Completes the login, getting any Steam Guard code from
    /// `authenticator` and asking again while a code is wrong.
    pub async fn wait_with(&mut self, authenticator: &mut impl Authenticator) -> Result<AuthTokens> {
        let confirmations = &self.poller.allowed_confirmations;
        let mut preferred = confirmations
            .first()
            .filter(|c| c.confirmation_type() != EAuthSessionGuardType::KEAuthSessionGuardTypeUnknown)
            .cloned()
            .ok_or_else(|| Error::Other("there are no allowed confirmations".into()))?;

        if preferred.confirmation_type() == EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceConfirmation
            && !authenticator.accept_device_confirmation().await
        {
            preferred = confirmations.get(1).cloned().ok_or_else(|| {
                Error::Other("device confirmation was declined and there is no other confirmation type".into())
            })?;
        }

        let (code_type, invalid) = match preferred.confirmation_type() {
            EAuthSessionGuardType::KEAuthSessionGuardTypeEmailCode => {
                (GuardCodeType::Email, EResult::InvalidLoginAuthCode)
            }
            EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceCode => {
                (GuardCodeType::Device, EResult::TwoFactorCodeMismatch)
            }
            // Nothing to enter: wait for the session (or the confirmation in
            // the app or the email link) to complete.
            EAuthSessionGuardType::KEAuthSessionGuardTypeNone
            | EAuthSessionGuardType::KEAuthSessionGuardTypeDeviceConfirmation
            | EAuthSessionGuardType::KEAuthSessionGuardTypeEmailConfirmation => return self.wait().await,
            other => return Err(Error::Other(format!("unsupported confirmation type {other:?}"))),
        };

        let mut previous_incorrect = false;
        loop {
            let code = match code_type {
                GuardCodeType::Email => {
                    authenticator.email_code(preferred.associated_message(), previous_incorrect).await?
                }
                GuardCodeType::Device => authenticator.device_code(previous_incorrect).await?,
            };
            if code.is_empty() {
                return Err(Error::Other("the authenticator provided no code".into()));
            }
            match self.submit_code(&code, code_type).await {
                Ok(()) => return self.wait().await,
                Err(e) if e.result() == Some(invalid) => previous_incorrect = true,
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmations_are_sorted() {
        use EAuthSessionGuardType::*;
        let mut list: Vec<_> = [
            KEAuthSessionGuardTypeEmailCode,
            KEAuthSessionGuardTypeDeviceConfirmation,
            KEAuthSessionGuardTypeDeviceCode,
        ]
        .into_iter()
        .map(|t| CAuthenticationAllowedConfirmation { confirmation_type: Some(t as i32), associated_message: None })
        .collect();
        sort_confirmations(&mut list);
        let order: Vec<_> = list.iter().map(|c| c.confirmation_type()).collect();
        assert_eq!(
            order,
            [
                KEAuthSessionGuardTypeDeviceConfirmation,
                KEAuthSessionGuardTypeDeviceCode,
                KEAuthSessionGuardTypeEmailCode
            ]
        );
    }
}
