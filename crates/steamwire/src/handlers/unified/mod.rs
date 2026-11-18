//! Unified messages: calls to `Service.Method#version` RPCs over the CM
//! connection (SteamKit2's `SteamUnifiedMessages`).
//!
//! The typed clients in [`crate::services`] (SteamKit2's generated service
//! classes) go through [`SteamClient::call_service`] and
//! [`SteamClient::send_service_notification`]:
//!
//! ```no_run
//! # async fn run(client: steamwire::SteamClient) -> steamwire::Result<()> {
//! use steamwire::protos::steam::CPlayerGetGameBadgeLevelsRequest;
//! use steamwire::services::Player;
//!
//! let levels = Player::new(&client)
//!     .get_game_badge_levels(CPlayerGetGameBadgeLevelsRequest { appid: Some(440) })
//!     .await?;
//! # Ok(()) }
//! ```

use std::future::Future;

use prost::Message;

use crate::client::SteamClient;
use crate::enums::{EMsg, EResult};
use crate::error::{Error, Result};
use crate::messages::{Packet, ProtoMsg};
use crate::protos::ServiceTransport;

/// Turns the header result of a service response into an error.
pub(crate) fn check_result(packet: &Packet) -> Result<()> {
    let result = packet.header_result().unwrap_or(EResult::OK);
    if result == EResult::OK {
        return Ok(());
    }
    let message = packet.proto_header().and_then(|h| h.error_message.clone()).unwrap_or_default();
    if message.is_empty() {
        Err(Error::Steam(result))
    } else {
        Err(Error::SteamMessage { result, message })
    }
}

impl SteamClient {
    fn service_emsg(&self) -> EMsg {
        if self.steam_id().is_some() {
            EMsg::ServiceMethodCallFromClient
        } else {
            EMsg::ServiceMethodCallFromClientNonAuthed
        }
    }

    /// Calls a service method and decodes its response.
    ///
    /// `method` has the form `Service.Method#version`, for example
    /// `"Player.GetGameBadgeLevels#1"`. Before logon the call is sent as
    /// `ServiceMethodCallFromClientNonAuthed`.
    ///
    /// ```no_run
    /// # async fn run(client: steamwire::SteamClient) -> steamwire::Result<()> {
    /// use steamwire::protos::steam::{CPlayerGetGameBadgeLevelsRequest, CPlayerGetGameBadgeLevelsResponse};
    ///
    /// let levels: CPlayerGetGameBadgeLevelsResponse = client
    ///     .call_service("Player.GetGameBadgeLevels#1", CPlayerGetGameBadgeLevelsRequest { appid: Some(440) })
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub async fn call_service<Req, Resp>(&self, method: &str, request: Req) -> Result<Resp>
    where
        Req: Message,
        Resp: Message + Default,
    {
        let mut msg = ProtoMsg::new(self.service_emsg(), request);
        msg.header.target_job_name = Some(method.to_string());
        let packet = self.call(msg).await?;
        check_result(&packet)?;
        packet.decode()
    }

    /// Sends a service notification (no response expected).
    pub fn send_service_notification<Req: Message>(&self, method: &str, request: Req) -> Result<()> {
        let mut msg = ProtoMsg::new(self.service_emsg(), request);
        msg.header.target_job_name = Some(method.to_string());
        self.send(msg)
    }
}

impl ServiceTransport for SteamClient {
    type Error = Error;

    fn call<Req, Resp>(&self, method: &'static str, request: Req) -> impl Future<Output = Result<Resp>> + Send
    where
        Req: Message + Send + 'static,
        Resp: Message + Default + Send + 'static,
    {
        self.call_service(method, request)
    }

    fn notify<Req>(&self, method: &'static str, request: Req) -> Result<()>
    where
        Req: Message + Send + 'static,
    {
        self.send_service_notification(method, request)
    }
}

/// The method name of an incoming service notification (`EMsg::ServiceMethod`).
pub fn notification_method(packet: &Packet) -> Option<&str> {
    if packet.emsg != EMsg::ServiceMethod {
        return None;
    }
    packet.proto_header()?.target_job_name.as_deref()
}

/// An incoming service notification (`EMsg::ServiceMethod`), such as
/// `FriendMessagesClient.IncomingMessage#1`. Decode the body with
/// [`ServiceNotification::decode`], or by method with the service's
/// notification enum:
///
/// ```
/// # fn run(notification: steamwire::handlers::unified::ServiceNotification) -> steamwire::Result<()> {
/// use steamwire::services::FriendMessagesClientNotification as Notification;
///
/// match Notification::decode(&notification.method, &notification.packet.body).transpose()? {
///     Some(Notification::IncomingMessage(message)) => println!("{}", message.message()),
///     Some(_) | None => {}
/// }
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct ServiceNotification {
    /// The method, `Service.Method#version`.
    pub method: String,
    /// The packet.
    pub packet: Packet,
}

impl ServiceNotification {
    /// Wraps a `ServiceMethod` packet.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        let method = notification_method(packet)?.to_string();
        Some(Self { method, packet: packet.clone() })
    }

    /// The service name (`FriendMessagesClient`).
    pub fn service(&self) -> &str {
        self.method.split('.').next().unwrap_or_default()
    }

    /// The method name without service or version (`IncomingMessage`).
    pub fn method_name(&self) -> &str {
        let rest = self.method.split_once('.').map_or(self.method.as_str(), |(_, m)| m);
        rest.split('#').next().unwrap_or_default()
    }

    /// Decodes the notification body.
    pub fn decode<M: Message + Default>(&self) -> Result<M> {
        self.packet.decode()
    }
}

#[cfg(test)]
mod tests {
    use crate::protos::steam::CPlayerGetGameBadgeLevelsRequest;
    use crate::services::Player;
    use crate::SteamClient;

    // The typed clients' futures can be spawned on a multi-threaded runtime.
    #[allow(dead_code)]
    fn service_futures_are_send(client: &SteamClient) {
        fn assert_send<F: Send>(_: F) {}
        assert_send(Player::new(client).get_game_badge_levels(CPlayerGetGameBadgeLevelsRequest::default()));
    }
}
