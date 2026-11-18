//! Logging on and off (SteamKit2's `SteamUser`).

mod callbacks;

pub use callbacks::*;

use std::net::{IpAddr, Ipv4Addr};

use steamwire_protos::steam::{
    c_msg_ip_address, CMsgClientLogOff, CMsgClientLogon, CMsgClientLogonResponse, CMsgIpAddress,
};

use crate::client::{SteamClient, CURRENT_PROTOCOL};
use crate::enums::{EAccountType, EGamingDeviceType, EMsg, EOSType, EResult, EUIMode};
use crate::error::{Error, Result};
use crate::messages::ProtoMsg;
use crate::types::SteamId;
use crate::util::{hardware, net, os};

/// The chat mode sent at logon (SteamKit2's `SteamUser.ChatMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatMode {
    /// The default chat mode.
    #[default]
    Default,
    /// The chat mode of the current Steam client.
    NewSteamChat,
}

/// Details for [`SteamClient::log_on`].
#[derive(Clone, Debug)]
pub struct LogOnDetails {
    /// The account name.
    pub username: String,
    /// The account ID, 0 if unknown.
    pub account_id: u32,
    /// The account instance; [`SteamId::DESKTOP_INSTANCE`] by default.
    pub account_instance: u32,
    /// The refresh token from an [auth session](crate::handlers::auth). Steam
    /// calls this field `access_token` in the logon message.
    pub access_token: Option<String>,
    /// The password, for the legacy logon flow. Prefer `access_token`.
    pub password: Option<String>,
    /// Whether Steam should issue a login key.
    pub should_remember_password: bool,
    /// The Steam Guard code from email, for the legacy logon flow.
    pub auth_code: Option<String>,
    /// The Steam Guard code from the mobile authenticator, for the legacy
    /// logon flow.
    pub two_factor_code: Option<String>,
    /// Whether to ask for a Steam2 ticket.
    pub request_steam2_ticket: bool,
    /// The OS reported to Steam.
    pub client_os_type: EOSType,
    /// The client language, e.g. `"english"`.
    pub client_language: String,
    /// The cell ID; `None` uses the client's (see [`Config::cell_id`](crate::Config::cell_id)).
    pub cell_id: Option<u32>,
    /// The machine name shown in the account's device list.
    pub machine_name: String,
    /// The login ID ("obfuscated private IP"). Steam allows one session per
    /// login ID, account and public IP, so concurrent sessions need distinct
    /// IDs. `None` sends the connection's local IP, as SteamKit2 does.
    pub login_id: Option<u32>,
    /// The chat mode.
    pub chat_mode: ChatMode,
    /// The UI mode; sent unless `EUIMode::Unknown`.
    pub ui_mode: EUIMode,
    /// The gaming device type; sent unless `EGamingDeviceType::Unknown`.
    pub gaming_device_type: EGamingDeviceType,
}

impl Default for LogOnDetails {
    fn default() -> Self {
        Self {
            username: String::new(),
            account_id: 0,
            account_instance: SteamId::DESKTOP_INSTANCE,
            access_token: None,
            password: None,
            should_remember_password: false,
            auth_code: None,
            two_factor_code: None,
            request_steam2_ticket: false,
            client_os_type: default_os_type(),
            client_language: "english".into(),
            cell_id: None,
            machine_name: hardware::machine_name(),
            login_id: None,
            chat_mode: ChatMode::Default,
            ui_mode: EUIMode::Unknown,
            gaming_device_type: EGamingDeviceType::Unknown,
        }
    }
}

/// Details for [`SteamClient::log_on_anonymous_with`].
#[derive(Clone, Debug)]
pub struct AnonymousLogOnDetails {
    /// The cell ID; `None` uses the client's (see [`Config::cell_id`](crate::Config::cell_id)).
    pub cell_id: Option<u32>,
    /// The OS reported to Steam.
    pub client_os_type: EOSType,
    /// The client language, e.g. `"english"`.
    pub client_language: String,
}

impl Default for AnonymousLogOnDetails {
    fn default() -> Self {
        Self { cell_id: None, client_os_type: default_os_type(), client_language: "english".into() }
    }
}

/// The OS type of the machine running this code (see [`os::os_type`]).
pub fn default_os_type() -> EOSType {
    os::os_type()
}

/// The result of a successful logon.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct LoggedOn {
    /// The Steam ID of the session.
    pub steam_id: SteamId,
    /// The raw response.
    pub response: CMsgClientLogonResponse,
}

impl SteamClient {
    async fn send_logon(&self, steam_id: SteamId, body: CMsgClientLogon) -> Result<LoggedOn> {
        self.send_logon_as(EMsg::ClientLogon, steam_id, body).await
    }

    pub(crate) async fn send_logon_as(&self, emsg: EMsg, steam_id: SteamId, body: CMsgClientLogon) -> Result<LoggedOn> {
        let mut msg = ProtoMsg::new(emsg, body);
        msg.header.steamid = Some(steam_id.as_u64());
        msg.header.client_sessionid = Some(0);

        let packet = self.send_and_wait(msg, EMsg::ClientLogOnResponse).await?;
        if !packet.is_proto() {
            return Err(Error::Other("non-protobuf logon response".into()));
        }
        let response: CMsgClientLogonResponse = packet.decode()?;
        let result = EResult(response.eresult());
        if result != EResult::OK {
            return Err(Error::Steam(result));
        }
        Ok(LoggedOn { steam_id: packet.steam_id().unwrap_or_default(), response })
    }

    /// The logon's `obfuscated_private_ip`: the connection's local IP, or
    /// `255.255.255.255` (`IPAddress.None`) over WebSocket, as in SteamKit2.
    pub(crate) fn obfuscated_local_ip(&self) -> CMsgIpAddress {
        let local_ip = self.local_ip().unwrap_or(IpAddr::V4(Ipv4Addr::BROADCAST));
        net::obfuscate_private_ip(net::ip_to_msg(local_ip))
    }

    /// Logs on to an anonymous user account. Enough for PICS, the CDN and
    /// many unified messages.
    pub async fn log_on_anonymous(&self) -> Result<LoggedOn> {
        self.log_on_anonymous_with(AnonymousLogOnDetails::default()).await
    }

    /// Logs on to an anonymous user account with the given details.
    pub async fn log_on_anonymous_with(&self, details: AnonymousLogOnDetails) -> Result<LoggedOn> {
        let steam_id = SteamId::new(0, 0, self.universe(), EAccountType::AnonUser);
        let body = CMsgClientLogon {
            protocol_version: Some(CURRENT_PROTOCOL),
            client_os_type: Some(details.client_os_type.0 as u32),
            client_language: Some(details.client_language),
            cell_id: Some(details.cell_id.unwrap_or_else(|| self.effective_cell_id())),
            machine_id: Some(self.machine_id()),
            ..Default::default()
        };
        self.send_logon(steam_id, body).await
    }

    /// Logs on to a user account, normally with the refresh token from
    /// [`QrAuthSession`](crate::handlers::auth::QrAuthSession) or
    /// [`CredentialsAuthSession`](crate::handlers::auth::CredentialsAuthSession).
    pub async fn log_on(&self, details: LogOnDetails) -> Result<LoggedOn> {
        if details.username.is_empty() || (details.password.is_none() && details.access_token.is_none()) {
            return Err(Error::Other("log_on needs a username and an access token or password".into()));
        }
        let steam_id =
            SteamId::new(details.account_id, details.account_instance, self.universe(), EAccountType::Individual);
        // An explicit login ID is sent as is; the local IP is obfuscated.
        let obfuscated_private_ip = match details.login_id {
            Some(login_id) => CMsgIpAddress { ip: Some(c_msg_ip_address::Ip::V4(login_id)) },
            None => self.obfuscated_local_ip(),
        };
        let deprecated_obfustucated_private_ip = match obfuscated_private_ip.ip {
            Some(c_msg_ip_address::Ip::V4(v4)) => Some(v4),
            _ => None,
        };
        let body = CMsgClientLogon {
            protocol_version: Some(CURRENT_PROTOCOL),
            client_os_type: Some(details.client_os_type.0 as u32),
            client_language: Some(details.client_language),
            cell_id: Some(details.cell_id.unwrap_or_else(|| self.effective_cell_id())),
            account_name: Some(details.username),
            password: details.password,
            access_token: details.access_token,
            should_remember_password: Some(details.should_remember_password),
            auth_code: details.auth_code,
            two_factor_code: details.two_factor_code,
            steam2_ticket_request: Some(details.request_steam2_ticket),
            obfuscated_private_ip: Some(obfuscated_private_ip),
            deprecated_obfustucated_private_ip,
            chat_mode: match details.chat_mode {
                ChatMode::Default => None,
                ChatMode::NewSteamChat => Some(2),
            },
            ui_mode: (details.ui_mode != EUIMode::Unknown).then_some(details.ui_mode.0 as u32),
            gaming_device_type: (details.gaming_device_type != EGamingDeviceType::Unknown)
                .then_some(details.gaming_device_type.0 as u32),
            client_package_version: Some(1771),
            supports_rate_limit_response: Some(true),
            machine_name: Some(details.machine_name),
            machine_id: Some(self.machine_id()),
            ..Default::default()
        };
        self.send_logon(steam_id, body).await
    }

    /// Logs off. The server then closes the connection, reported as an
    /// expected [`Event::Disconnected`](crate::Event::Disconnected).
    pub fn log_off(&self) -> Result<()> {
        self.expect_disconnect();
        self.send(ProtoMsg::new(EMsg::ClientLogOff, CMsgClientLogOff::default()))
    }
}
