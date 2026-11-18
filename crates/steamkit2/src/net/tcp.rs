//! `VT01` framed TCP. Channel encryption is added by [`super::envelope`].

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::{Connection, Outgoing};
use crate::error::{Error, Result};

const TCP_MAGIC: u32 = 0x3130_5456; // "VT01"
const MAX_FRAME: usize = 64 * 1024 * 1024;

async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Vec<u8>> {
    let len = r.read_u32_le().await? as usize;
    let magic = r.read_u32_le().await?;
    if magic != TCP_MAGIC {
        return Err(Error::Malformed("bad TCP frame magic"));
    }
    if len > MAX_FRAME {
        return Err(Error::Malformed("TCP frame too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, data: &[u8]) -> Result<()> {
    let mut frame = Vec::with_capacity(8 + data.len());
    frame.extend_from_slice(&(data.len() as u32).to_le_bytes());
    frame.extend_from_slice(&TCP_MAGIC.to_le_bytes());
    frame.extend_from_slice(data);
    w.write_all(&frame).await?;
    Ok(())
}

/// Connects and returns the plain framed message link.
pub(super) async fn connect(endpoint: &str) -> Result<Connection> {
    let stream = TcpStream::connect(endpoint).await?;
    stream.set_nodelay(true)?;
    let local_ip = stream.local_addr().ok().map(|a| a.ip());

    let (mut reader, mut writer) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Outgoing>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<Bytes>();

    tokio::spawn(async move {
        while let Some(Outgoing::Data(data)) = out_rx.recv().await {
            if let Err(e) = write_frame(&mut writer, &data).await {
                tracing::debug!("TCP send failed: {e}");
                return;
            }
        }
        let _ = writer.shutdown().await;
    });

    tokio::spawn(async move {
        loop {
            match read_frame(&mut reader).await {
                Ok(frame) => {
                    if in_tx.send(Bytes::from(frame)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::debug!("TCP receive ended: {e}");
                    return;
                }
            }
        }
    });

    Ok(Connection { outgoing: out_tx, incoming: in_rx, local_ip })
}
