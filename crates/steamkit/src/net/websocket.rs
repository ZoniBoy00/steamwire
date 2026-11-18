//! WebSocket transport: `wss://host:port/cmsocket/`, one binary frame per message.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{Connection, Outgoing};
use crate::error::Result;

pub(super) async fn connect(endpoint: &str) -> Result<Connection> {
    let url = format!("wss://{endpoint}/cmsocket/");
    let (stream, _) = tokio_tungstenite::connect_async(url).await?;
    let (mut sink, mut stream) = stream.split();

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outgoing>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Bytes>();

    tokio::spawn(async move {
        while let Some(Outgoing::Data(data)) = out_rx.recv().await {
            if let Err(e) = sink.send(WsMessage::Binary(data.into())).await {
                tracing::debug!("websocket send failed: {e}");
                return;
            }
        }
        let _ = sink.close().await;
    });

    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(WsMessage::Binary(data)) => {
                    if in_tx.send(data).is_err() {
                        return;
                    }
                }
                Ok(WsMessage::Close(frame)) => {
                    tracing::debug!(?frame, "websocket closed by server");
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!("websocket receive failed: {e}");
                    return;
                }
            }
        }
    });

    Ok(Connection { outgoing: out_tx, incoming: in_rx, local_ip: None })
}
