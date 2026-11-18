//! Channel encryption on top of a message link (SteamKit2's
//! `EnvelopeEncryptedConnection`), shared by the TCP and UDP transports.

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::mpsc;

use super::encryption::{self as crypto, ChannelCipher};
use super::{Connection, NetworkListener, Outgoing};
use crate::enums::{EMsg, EResult, EUniverse};
use crate::error::{Error, Result};
use crate::messages::{Header, MsgHdr, Packet};
use crate::types::JobId;

type Listener = Option<Arc<dyn NetworkListener>>;

async fn recv(link: &mut Connection, listener: &Listener) -> Result<Packet> {
    let data = link.incoming.recv().await.ok_or(Error::NotConnected)?;
    let packet = Packet::parse(data.clone())?;
    if let Some(l) = listener {
        l.on_incoming(packet.emsg, &data);
    }
    Ok(packet)
}

fn send(link: &Connection, listener: &Listener, emsg: EMsg, data: Vec<u8>) -> Result<()> {
    if let Some(l) = listener {
        l.on_outgoing(emsg, &data);
    }
    link.outgoing.send(Outgoing::Data(data)).map_err(|_| Error::NotConnected)
}

/// Runs the `ChannelEncryptRequest` / `Response` / `Result` handshake and
/// returns the session cipher.
async fn handshake(link: &mut Connection, universe: EUniverse, listener: &Listener) -> Result<ChannelCipher> {
    let request = recv(link, listener).await?;
    if request.emsg != EMsg::ChannelEncryptRequest {
        return Err(Error::UnexpectedMessage(request.emsg));
    }
    if request.body.len() < 8 + 16 {
        return Err(Error::Malformed("short ChannelEncryptRequest"));
    }
    let request_universe = EUniverse(i32::from_le_bytes(request.body[4..8].try_into().unwrap()));
    if request_universe != universe {
        return Err(Error::Other(format!("server is in universe {request_universe:?}")));
    }
    let challenge = &request.body[8..];

    let public_key =
        crypto::universe_public_key(universe).ok_or_else(|| Error::Other(format!("no public key for {universe:?}")))?;
    let session_key: [u8; 32] = crate::util::crypto::random_bytes();
    let mut blob = session_key.to_vec();
    blob.extend_from_slice(challenge);
    let encrypted = crypto::rsa_encrypt_oaep_sha1(public_key, &blob)?;

    let mut body = Vec::with_capacity(8 + encrypted.len() + 8);
    body.extend_from_slice(&1u32.to_le_bytes()); // protocol version
    body.extend_from_slice(&128u32.to_le_bytes()); // key size
    body.extend_from_slice(&encrypted);
    body.extend_from_slice(&crc32fast::hash(&encrypted).to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    let response = Packet {
        emsg: EMsg::ChannelEncryptResponse,
        header: Header::Plain(MsgHdr { target_job: JobId::NONE, source_job: JobId::NONE }),
        body: Bytes::from(body),
    };
    send(link, listener, response.emsg, response.encode())?;

    let result = recv(link, listener).await?;
    if result.emsg != EMsg::ChannelEncryptResult {
        return Err(Error::UnexpectedMessage(result.emsg));
    }
    if result.body.len() < 4 {
        return Err(Error::Malformed("short ChannelEncryptResult"));
    }
    let eresult = EResult(i32::from_le_bytes(result.body[..4].try_into().unwrap()));
    if eresult != EResult::OK {
        return Err(Error::Steam(eresult));
    }
    Ok(ChannelCipher::new(&session_key))
}

/// Encrypts a plain message link: runs the handshake, then encrypts
/// outgoing and decrypts incoming messages.
pub(super) async fn wrap(mut link: Connection, universe: EUniverse, listener: Listener) -> Result<Connection> {
    let cipher = handshake(&mut link, universe, &listener).await?;
    tracing::debug!("channel encrypted");

    let Connection { outgoing: link_out, incoming: mut link_in, local_ip } = link;
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outgoing>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Bytes>();

    let send_cipher = cipher.clone();
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let msg = match msg {
                Outgoing::Data(data) => Outgoing::Data(send_cipher.encrypt(&data)),
                Outgoing::Close => Outgoing::Close,
            };
            let close = matches!(msg, Outgoing::Close);
            if link_out.send(msg).is_err() || close {
                return;
            }
        }
        let _ = link_out.send(Outgoing::Close);
    });

    tokio::spawn(async move {
        while let Some(data) = link_in.recv().await {
            match cipher.decrypt(&data) {
                Ok(plain) => {
                    if in_tx.send(Bytes::from(plain)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::warn!("dropping connection: {e}");
                    return;
                }
            }
        }
    });

    Ok(Connection { outgoing: out_tx, incoming: in_rx, local_ip })
}
