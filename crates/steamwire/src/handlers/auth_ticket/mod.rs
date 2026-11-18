//! Game auth session tickets (SteamKit2's `SteamAuthTicket`): tickets a game
//! client hands to a game server or web API for `BeginAuthSession` /
//! `AuthenticateUserTicket`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use steamwire_protos::steam::{
    CMsgAuthTicket, CMsgClientAuthList, CMsgClientAuthListAck, CMsgClientGameConnectTokens,
    CMsgClientTicketAuthComplete,
};
use tokio::sync::oneshot;

use crate::client::SteamClient;
use crate::enums::EMsg;
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};

/// The size of a web API ticket (`m_rgubTicket`).
const WEB_API_TICKET_SIZE: usize = 2560;
/// How long to wait for Steam to acknowledge a ticket.
const TICKET_ACK_TIMEOUT: Duration = Duration::from_secs(10);

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamAuthTicket`.
    pub enum AuthTicketCallback {
        /// `ClientAuthListAck`: Steam accepted our active tickets.
        TicketAccepted(CMsgClientAuthListAck) = ClientAuthListAck,
        /// `ClientTicketAuthComplete`: a ticket was used.
        TicketAuthComplete(CMsgClientTicketAuthComplete) = ClientTicketAuthComplete,
    }
}

impl AuthTicketCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

/// The kind of ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum TicketType {
    /// For `BeginAuthSession` on a game server.
    AuthSession = 2,
    /// For `ISteamUserAuth/AuthenticateUserTicket`.
    WebApiTicket = 5,
}

#[derive(Debug, Default)]
struct State {
    tokens: VecDeque<Vec<u8>>,
    tickets: HashMap<u32, Vec<CMsgAuthTicket>>,
    acks: HashMap<u32, oneshot::Sender<()>>,
}

/// Game connect tokens and active tickets.
#[derive(Debug, Default)]
pub(crate) struct AuthTicketState {
    state: Mutex<State>,
}

impl AuthTicketState {
    pub(crate) fn observe(&self, packet: &Packet) {
        match packet.emsg {
            EMsg::ClientGameConnectTokens => {
                if let Ok(body) = packet.decode::<CMsgClientGameConnectTokens>() {
                    let mut state = self.state.lock().unwrap();
                    let keep = body.max_tokens_to_keep() as usize;
                    state.tokens.extend(body.tokens);
                    while state.tokens.len() > keep {
                        state.tokens.pop_front();
                    }
                }
            }
            EMsg::ClientAuthListAck => {
                // Matched by CRC: Steam doesn't reliably echo the job ID on
                // repeated auth lists (SteamKit issue #1567).
                if let Ok(body) = packet.decode::<CMsgClientAuthListAck>() {
                    let mut state = self.state.lock().unwrap();
                    for crc in body.ticket_crc {
                        if let Some(tx) = state.acks.remove(&crc) {
                            let _ = tx.send(());
                        }
                    }
                }
            }
            EMsg::ClientLoggedOff => self.state.lock().unwrap().tokens.clear(),
            _ => {}
        }
    }
}

/// An active ticket. Dropping it cancels the ticket.
#[derive(Debug)]
pub struct TicketInfo {
    client: SteamClient,
    app_id: u32,
    auth_crc: u32,
    /// The ticket to hand to the server.
    pub ticket: Vec<u8>,
}

impl Drop for TicketInfo {
    fn drop(&mut self) {
        let state = &self.client.handler_state().auth_tickets;
        {
            let mut state = state.state.lock().unwrap();
            if let Some(tickets) = state.tickets.get_mut(&self.app_id) {
                tickets.retain(|t| t.ticket_crc() != self.auth_crc);
            }
        }
        let _ = self.client.send_auth_list();
    }
}

fn build_auth_ticket(token: &[u8], ticket_type: TicketType) -> Vec<u8> {
    const SESSION_SIZE: u32 = 4 * 6;
    let mut out = Vec::with_capacity(token.len() + 8 + SESSION_SIZE as usize);
    out.extend_from_slice(&(token.len() as u32).to_le_bytes());
    out.extend_from_slice(token);
    out.extend_from_slice(&SESSION_SIZE.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(ticket_type as u32).to_le_bytes());
    out.extend_from_slice(&crate::util::crypto::random_bytes::<8>());
    let ticks = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u32;
    out.extend_from_slice(&ticks.to_le_bytes());
    out.extend_from_slice(&(SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1).to_le_bytes());
    out
}

fn combine_tickets(auth_ticket: &[u8], app_ticket: &[u8], pad: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(WEB_API_TICKET_SIZE.max(auth_ticket.len() + 4 + app_ticket.len()));
    out.extend_from_slice(auth_ticket);
    out.extend_from_slice(&(app_ticket.len() as u32).to_le_bytes());
    out.extend_from_slice(app_ticket);
    if pad && out.len() < WEB_API_TICKET_SIZE {
        // The real client sends the whole buffer; the rest is junk.
        let mut junk = vec![0u8; WEB_API_TICKET_SIZE - out.len()];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut junk);
        out.extend_from_slice(&junk);
    }
    out
}

impl SteamClient {
    fn send_auth_list(&self) -> Result<()> {
        let body = {
            let state = self.handler_state().auth_tickets.state.lock().unwrap();
            CMsgClientAuthList {
                tokens_left: Some(state.tokens.len() as u32),
                app_ids: state.tickets.keys().copied().collect(),
                tickets: state.tickets.values().flatten().cloned().collect(),
                ..Default::default()
            }
        };
        let mut msg = ProtoMsg::new(EMsg::ClientAuthList, body);
        msg.header.jobid_source = Some(self.new_job_id().0);
        self.send(msg)
    }

    /// Creates an auth session ticket for a game server.
    pub async fn get_auth_session_ticket(&self, app_id: u32) -> Result<TicketInfo> {
        self.auth_ticket(app_id, TicketType::AuthSession, None).await
    }

    /// Creates a ticket for `AuthenticateUserTicket`, bound to an identity.
    pub async fn get_auth_ticket_for_web_api(&self, app_id: u32, identity: &str) -> Result<TicketInfo> {
        self.auth_ticket(app_id, TicketType::WebApiTicket, Some(identity)).await
    }

    async fn auth_ticket(&self, app_id: u32, ticket_type: TicketType, identity: Option<&str>) -> Result<TicketInfo> {
        if self.cell_id().is_none() {
            return Err(Error::NotConnected);
        }
        let app_ticket = self.get_app_ownership_ticket(app_id).await?.ticket.unwrap_or_default();
        let state = &self.handler_state().auth_tickets;
        let token = state
            .state
            .lock()
            .unwrap()
            .tokens
            .pop_front()
            .ok_or_else(|| Error::Other("no game connect tokens left".into()))?;

        let auth_ticket = build_auth_ticket(&token, ticket_type);
        let crc = crc32fast::hash(&auth_ticket);
        // Steam prefixes the identity with "str:" and NUL terminates it.
        let server_secret = identity.filter(|i| !i.is_empty()).map(|i| format!("str:{i}\0").into_bytes());
        let (tx, rx) = oneshot::channel();
        {
            let mut s = state.state.lock().unwrap();
            s.tickets.entry(app_id).or_default().push(CMsgAuthTicket {
                gameid: Some(app_id as u64),
                ticket: Some(auth_ticket.clone()),
                ticket_crc: Some(crc),
                server_secret,
                ..Default::default()
            });
            s.acks.insert(crc, tx);
        }
        self.send_auth_list()?;
        // A missing ack isn't fatal: the ticket was submitted and is checked
        // server side.
        if tokio::time::timeout(TICKET_ACK_TIMEOUT, rx).await.is_err() {
            tracing::debug!(crc, "auth ticket was not acknowledged");
        }
        state.state.lock().unwrap().acks.remove(&crc);

        Ok(TicketInfo {
            client: self.clone(),
            app_id,
            auth_crc: crc,
            ticket: combine_tickets(&auth_ticket, &app_ticket, ticket_type == TicketType::WebApiTicket),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_layout() {
        let t = build_auth_ticket(&[7; 8], TicketType::WebApiTicket);
        assert_eq!(t.len(), 4 + 8 + 4 + 24);
        assert_eq!(&t[..4], &8u32.to_le_bytes());
        assert_eq!(&t[12..16], &24u32.to_le_bytes());
        assert_eq!(&t[20..24], &5u32.to_le_bytes());
        let combined = combine_tickets(&t, &[1, 2, 3], true);
        assert_eq!(combined.len(), WEB_API_TICKET_SIZE);
        assert_eq!(&combined[t.len()..t.len() + 4], &3u32.to_le_bytes());
        assert_eq!(combine_tickets(&t, &[1, 2, 3], false).len(), t.len() + 7);
    }
}
