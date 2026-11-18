//! The CM client: connection, message dispatch, jobs and heartbeats.

mod config;
mod events;
mod job;

pub use config::Config;
pub use events::{Event, Events};
pub use job::JobReceiver;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use steamkit2_protos::steam::{
    CMsgClientHeartBeat, CMsgClientHello, CMsgClientLoggedOff, CMsgClientLogonResponse, CMsgClientSessionToken,
    CMsgIpAddress,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::discovery::{ServerList, ServerQuality};
use crate::enums::{EClientPersonaStateFlag, EMsg, EResult, EUniverse};
use crate::error::{Error, Result};
use crate::handlers::HandlerState;
use crate::messages::generated::MsgClientServerUnavailable;
use crate::messages::{split_multi_raw, ClientMsg, OutgoingMsg, Packet, ProtoMsg};
use crate::net::{self, CmServer, NetworkListener, Outgoing};
use crate::types::{JobId, SteamId};
use crate::util::hardware::{self, MachineInfoProvider};

/// `MsgClientLogon.CurrentProtocol` from SteamKit2.
pub const CURRENT_PROTOCOL: u32 = 65581;

/// How long [`SteamClient::call`] and friends wait for a response by
/// default ([`Config::job_timeout`]); SteamKit2's `AsyncJob.Timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Session {
    steam_id: Option<SteamId>,
    session_id: Option<i32>,
    cell_id: Option<u32>,
    session_token: Option<u64>,
    public_ip: Option<CMsgIpAddress>,
    ip_country_code: Option<String>,
    user_country_code: Option<String>,
    connected: bool,
    user_disconnect: bool,
}

struct Inner {
    universe: EUniverse,
    config_cell_id: u32,
    server_list: Arc<ServerList>,
    server: CmServer,
    local_ip: Option<std::net::IpAddr>,
    job_timeout: Duration,
    outgoing: mpsc::UnboundedSender<Outgoing>,
    events: mpsc::UnboundedSender<Event>,
    session: Mutex<Session>,
    jobs: Mutex<HashMap<u64, mpsc::UnboundedSender<Packet>>>,
    waiters: Mutex<Vec<(EMsg, tokio::sync::oneshot::Sender<Packet>)>>,
    next_job: AtomicU64,
    heartbeat: Mutex<Option<JoinHandle<()>>>,
    persona_flags: EClientPersonaStateFlag,
    handlers: HandlerState,
    listener: Option<Arc<dyn NetworkListener>>,
    machine_info: Arc<dyn MachineInfoProvider>,
    machine_id: OnceLock<Vec<u8>>,
}

/// A connection to a Steam CM server. Cheap to clone; all clones share the
/// connection.
///
/// ```no_run
/// # async fn run() -> steamkit2::Result<()> {
/// use steamkit2::{Config, SteamClient};
///
/// let (client, mut events) = SteamClient::connect(Config::default()).await?;
/// let response = client.log_on_anonymous().await?;
/// println!("logged on as {:?}", client.steam_id());
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct SteamClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SteamClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamClient")
            .field("server", &self.inner.server)
            .field("steam_id", &self.steam_id())
            .field("connected", &self.is_connected())
            .finish()
    }
}

impl SteamClient {
    /// Connects to a CM server and sends `ClientHello`.
    ///
    /// Without [`Config::server`], servers come from the Steam directory and
    /// the first one that accepts the connection is used.
    pub async fn connect(config: Config) -> Result<(SteamClient, Events)> {
        let servers = match &config.server {
            Some(server) => vec![server.clone()],
            None => {
                config.server_list.ensure_loaded().await?;
                config.server_list.candidates(config.protocols)
            }
        };
        if servers.is_empty() {
            return Err(Error::Other("the server list is empty".into()));
        }

        let mut last_error = None;
        for server in servers.iter().take(config.connect_attempts.max(1)) {
            let attempt =
                net::connect(server, config.universe, config.network_listener.clone(), config.connection_timeout);
            match attempt.await {
                Ok(conn) => {
                    config.server_list.mark(server, ServerQuality::Good);
                    return Ok(Self::start(conn, server.clone(), &config));
                }
                Err(e) => {
                    tracing::warn!(endpoint = %server.endpoint, "connect failed: {e}");
                    config.server_list.mark(server, ServerQuality::Bad);
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.expect("at least one attempt"))
    }

    fn start(conn: net::Connection, server: CmServer, config: &Config) -> (SteamClient, Events) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            universe: config.universe,
            config_cell_id: config.cell_id,
            server_list: config.server_list.clone(),
            server,
            local_ip: conn.local_ip,
            job_timeout: config.job_timeout,
            outgoing: conn.outgoing,
            events: event_tx,
            session: Mutex::new(Session { connected: true, ..Session::default() }),
            jobs: Mutex::new(HashMap::new()),
            waiters: Mutex::new(Vec::new()),
            next_job: AtomicU64::new(1),
            heartbeat: Mutex::new(None),
            persona_flags: config.persona_state_flags,
            handlers: HandlerState::default(),
            listener: config.network_listener.clone(),
            machine_info: config.machine_info_provider.clone(),
            machine_id: OnceLock::new(),
        });
        let client = SteamClient { inner };

        // The reader holds a weak handle: once every `SteamClient` is
        // dropped, `Inner` (and with it the outgoing sender) goes away and the
        // writer task closes the socket.
        let reader = Arc::downgrade(&client.inner);
        let mut incoming = conn.incoming;
        tokio::spawn(async move {
            while let Some(data) = incoming.recv().await {
                let Some(inner) = reader.upgrade() else { return };
                let client = SteamClient { inner };
                client.dispatch_raw(data);
            }
            if let Some(inner) = reader.upgrade() {
                SteamClient { inner }.on_disconnected();
            }
        });

        let hello = ProtoMsg::new(EMsg::ClientHello, CMsgClientHello { protocol_version: Some(CURRENT_PROTOCOL) });
        let _ = client.send(hello);

        (client, Events { rx: event_rx })
    }

    // ------------------------------------------------------------- state

    /// The universe.
    pub fn universe(&self) -> EUniverse {
        self.inner.universe
    }

    /// The server this client is connected to.
    pub fn server(&self) -> &CmServer {
        &self.inner.server
    }

    /// The Steam ID of the session, once logged on.
    pub fn steam_id(&self) -> Option<SteamId> {
        self.inner.session.lock().unwrap().steam_id
    }

    /// The session ID, once logged on.
    pub fn session_id(&self) -> Option<i32> {
        self.inner.session.lock().unwrap().session_id
    }

    /// The cell ID assigned at logon.
    pub fn cell_id(&self) -> Option<u32> {
        self.inner.session.lock().unwrap().cell_id
    }

    /// The cell ID to send: the one assigned at logon, else [`Config::cell_id`].
    /// The machine ID for logons, read from the provider once.
    pub(crate) fn machine_id(&self) -> Vec<u8> {
        self.inner.machine_id.get_or_init(|| hardware::machine_id(&*self.inner.machine_info)).clone()
    }

    pub(crate) fn effective_cell_id(&self) -> u32 {
        self.cell_id().unwrap_or(self.inner.config_cell_id)
    }

    /// Marks the next disconnect as expected (after a logoff).
    pub(crate) fn expect_disconnect(&self) {
        self.inner.session.lock().unwrap().user_disconnect = true;
    }

    /// The session token sent by the CM (`ClientSessionToken`).
    pub fn session_token(&self) -> Option<u64> {
        self.inner.session.lock().unwrap().session_token
    }

    /// The server list this client uses.
    pub fn server_list(&self) -> &Arc<ServerList> {
        &self.inner.server_list
    }

    /// Whether the connection is up.
    pub fn is_connected(&self) -> bool {
        self.inner.session.lock().unwrap().connected
    }

    /// Whether a logon succeeded on this connection.
    pub fn is_logged_on(&self) -> bool {
        self.inner.session.lock().unwrap().session_id.is_some()
    }

    // ----------------------------------------------------------- sending

    /// Sends a protobuf message. The Steam ID and session ID are filled in
    /// once logged on, unless the header already sets them.
    pub fn send(&self, mut msg: impl OutgoingMsg) -> Result<()> {
        {
            let session = self.inner.session.lock().unwrap();
            if !session.connected {
                return Err(Error::NotConnected);
            }
            msg.fill_session(session.steam_id, session.session_id);
        }
        tracing::trace!(emsg = ?msg.emsg(), "send");
        self.send_raw(msg.to_bytes())
    }

    /// Sends an already encoded message.
    pub fn send_raw(&self, data: Vec<u8>) -> Result<()> {
        if let Some(l) = &self.inner.listener {
            l.on_outgoing(net::raw_emsg(&data), &data);
        }
        self.inner.outgoing.send(Outgoing::Data(data)).map_err(|_| Error::NotConnected)
    }

    /// Allocates a new job ID.
    pub fn new_job_id(&self) -> JobId {
        JobId(self.inner.next_job.fetch_add(1, Ordering::Relaxed))
    }

    /// Sends a message as a job and returns a receiver for every reply that
    /// targets it. Use this for requests answered by several messages; the
    /// job ends when the [`JobReceiver`] is dropped.
    pub fn send_job(&self, mut msg: impl OutgoingMsg) -> Result<JobReceiver> {
        let job = self.new_job_id();
        msg.set_source_job(job);
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner.jobs.lock().unwrap().insert(job.0, tx);
        let receiver = JobReceiver { job, rx, timeout: self.inner.job_timeout, client: self.clone() };
        self.send(msg)?;
        Ok(receiver)
    }

    /// Sends a message as a job and waits for the first reply.
    pub async fn call(&self, msg: impl OutgoingMsg) -> Result<Packet> {
        let mut job = self.send_job(msg)?;
        job.recv().await
    }

    /// Sends a message and waits for the next message of type `reply`.
    /// For messages that are answered without a job ID, such as logon.
    pub async fn send_and_wait(&self, msg: impl OutgoingMsg, reply: EMsg) -> Result<Packet> {
        let wait = self.wait_for(reply);
        self.send(msg)?;
        wait.await
    }

    /// Waits for the next message of the given type. The message is not
    /// passed on to [`Events`].
    pub fn wait_for(&self, emsg: EMsg) -> impl std::future::Future<Output = Result<Packet>> + use<> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner.waiters.lock().unwrap().push((emsg, tx));
        let timeout = self.inner.job_timeout;
        async move {
            match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(packet)) => Ok(packet),
                Ok(Err(_)) => Err(Error::NotConnected),
                Err(_) => Err(Error::Timeout),
            }
        }
    }

    /// Closes the connection. An [`Event::Disconnected`] follows.
    pub fn disconnect(&self) {
        self.inner.session.lock().unwrap().user_disconnect = true;
        self.stop_heartbeat();
        let _ = self.inner.outgoing.send(Outgoing::Close);
    }

    // ---------------------------------------------------------- dispatch

    /// Our public IP as seen by Steam, once logged on.
    pub fn public_ip(&self) -> Option<CMsgIpAddress> {
        self.inner.session.lock().unwrap().public_ip.clone()
    }

    /// The local address of the connection's socket (SteamKit2's
    /// `LocalIP`). `None` over WebSocket, where SteamKit2 reports
    /// `IPAddress.None`.
    pub fn local_ip(&self) -> Option<std::net::IpAddr> {
        self.inner.local_ip
    }

    /// The country of our public IP, once logged on (`IPCountryCode`).
    pub fn ip_country_code(&self) -> Option<String> {
        self.inner.session.lock().unwrap().ip_country_code.clone()
    }

    /// The country of the account, once logged on (`UserCountryCode`).
    pub fn user_country_code(&self) -> Option<String> {
        self.inner.session.lock().unwrap().user_country_code.clone()
    }

    /// The job timeout ([`Config::job_timeout`]).
    pub fn job_timeout(&self) -> Duration {
        self.inner.job_timeout
    }

    pub(crate) fn handler_state(&self) -> &HandlerState {
        &self.inner.handlers
    }

    /// The persona flags requested by default ([`Config::persona_state_flags`]).
    pub fn default_persona_state_flags(&self) -> EClientPersonaStateFlag {
        self.inner.persona_flags
    }

    /// Parses and dispatches a received message. The listener sees the
    /// bytes as received; `Multi` messages are logged unpacked.
    fn dispatch_raw(&self, data: Bytes) {
        let packet = match Packet::parse(data.clone()) {
            Ok(packet) => packet,
            Err(e) => return tracing::warn!("dropping malformed packet: {e}"),
        };
        if packet.emsg == EMsg::Multi {
            match split_multi_raw(&packet) {
                Ok(messages) => messages.into_iter().for_each(|m| self.dispatch_raw(m)),
                Err(e) => tracing::warn!("bad Multi message: {e}"),
            }
            return;
        }
        if let Some(listener) = &self.inner.listener {
            listener.on_incoming(packet.emsg, &data);
        }
        self.dispatch(packet);
    }

    fn dispatch(&self, packet: Packet) {
        tracing::trace!(emsg = ?packet.emsg, "recv");
        match packet.emsg {
            EMsg::ClientLogOnResponse => self.handle_logon_response(&packet),
            EMsg::ClientLoggedOff => self.handle_logged_off(&packet),
            EMsg::ClientServerUnavailable => self.handle_server_unavailable(&packet),
            EMsg::ClientSessionToken => {
                if let Ok(body) = packet.decode::<CMsgClientSessionToken>() {
                    self.inner.session.lock().unwrap().session_token = body.token;
                }
            }
            _ => {}
        }
        if self.inner.handlers.observe(self, &packet) {
            return;
        }

        let target = packet.target_job();
        if !target.is_none() {
            let jobs = self.inner.jobs.lock().unwrap();
            if let Some(job) = jobs.get(&target.0) {
                if job.send(packet.clone()).is_ok() {
                    return;
                }
            }
        }

        {
            let mut waiters = self.inner.waiters.lock().unwrap();
            waiters.retain(|(_, tx)| !tx.is_closed());
            if let Some(i) = waiters.iter().position(|(emsg, _)| *emsg == packet.emsg) {
                let (_, tx) = waiters.remove(i);
                if tx.send(packet.clone()).is_ok() {
                    return;
                }
            }
        }

        let _ = self.inner.events.send(Event::Message(packet));
    }

    fn handle_logon_response(&self, packet: &Packet) {
        let Ok(body) = packet.decode::<CMsgClientLogonResponse>() else {
            return;
        };
        let result = EResult(body.eresult());
        if matches!(result, EResult::TryAnotherCM | EResult::ServiceUnavailable) {
            self.inner.server_list.mark(&self.inner.server, ServerQuality::Bad);
        }
        if result != EResult::OK {
            return;
        }
        {
            let mut session = self.inner.session.lock().unwrap();
            session.steam_id = packet.steam_id();
            session.session_id = packet.session_id();
            session.cell_id = body.cell_id;
            session.public_ip.clone_from(&body.public_ip);
            session.ip_country_code.clone_from(&body.ip_country_code);
            session.user_country_code.clone_from(&body.user_country);
        }
        let seconds = body.legacy_out_of_game_heartbeat_seconds.unwrap_or(9).max(1);
        self.start_heartbeat(Duration::from_secs(seconds as u64));
    }

    fn handle_logged_off(&self, packet: &Packet) {
        if let Ok(body) = packet.decode::<CMsgClientLoggedOff>() {
            let result = EResult(body.eresult());
            tracing::debug!(?result, "logged off");
            if matches!(result, EResult::TryAnotherCM | EResult::ServiceUnavailable) {
                self.inner.server_list.mark(&self.inner.server, ServerQuality::Bad);
            }
        }
        let mut session = self.inner.session.lock().unwrap();
        session.steam_id = None;
        session.session_id = None;
        session.cell_id = None;
        session.public_ip = None;
        session.ip_country_code = None;
        session.user_country_code = None;
        drop(session);
        self.stop_heartbeat();
    }

    fn handle_server_unavailable(&self, packet: &Packet) {
        if let Ok(msg) = ClientMsg::<MsgClientServerUnavailable>::from_packet(packet) {
            tracing::debug!(
                server_type = ?msg.body.e_server_type_unavailable,
                emsg_sent = ?EMsg(msg.body.e_msg_sent),
                "a server was unavailable for a request"
            );
        }
        self.inner.session.lock().unwrap().user_disconnect = false;
        let _ = self.inner.outgoing.send(Outgoing::Close);
    }

    fn start_heartbeat(&self, interval: Duration) {
        let weak = Arc::downgrade(&self.inner);
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let Some(inner) = weak.upgrade() else { return };
                let client = SteamClient { inner };
                if client.send(ProtoMsg::new(EMsg::ClientHeartBeat, CMsgClientHeartBeat::default())).is_err() {
                    return;
                }
            }
        });
        if let Some(old) = self.inner.heartbeat.lock().unwrap().replace(task) {
            old.abort();
        }
    }

    fn stop_heartbeat(&self) {
        if let Some(task) = self.inner.heartbeat.lock().unwrap().take() {
            task.abort();
        }
    }

    fn on_disconnected(&self) {
        let user_initiated = {
            let mut session = self.inner.session.lock().unwrap();
            let user = session.user_disconnect;
            *session = Session::default();
            user
        };
        self.stop_heartbeat();
        self.inner.jobs.lock().unwrap().clear();
        self.inner.waiters.lock().unwrap().clear();
        tracing::debug!(user_initiated, "disconnected");
        let _ = self.inner.events.send(Event::Disconnected { user_initiated });
    }
}

/// Builds a [`Packet`] from raw bytes; handy for tests and replaying dumps.
pub fn parse_packet(data: impl Into<Bytes>) -> Result<Packet> {
    Packet::parse(data.into())
}
