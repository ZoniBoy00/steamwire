//! Debug hooks for the plaintext messages of a connection (SteamKit2's
//! `IDebugNetworkListener` and `NetHookNetworkListener`).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::enums::EMsg;

/// Sees every message sent and received, unencrypted. `Multi` messages are
/// reported unpacked. Use this for debugging only.
pub trait NetworkListener: Send + Sync + fmt::Debug {
    /// A message was received.
    fn on_incoming(&self, emsg: EMsg, data: &[u8]);
    /// A message is being sent.
    fn on_outgoing(&self, emsg: EMsg, data: &[u8]);
}

/// The message type of a raw message.
pub(crate) fn raw_emsg(data: &[u8]) -> EMsg {
    let raw = data.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
    EMsg(raw & !crate::messages::PROTO_MASK)
}

/// Writes every message to a file in a directory, in the NetHook dump
/// format the NetHook analyzer reads: `001_in_751_k_EMsgClientLogOnResponse.bin`.
#[derive(Debug)]
pub struct NetHookListener {
    directory: PathBuf,
    message_number: AtomicU64,
}

impl NetHookListener {
    /// Logs to a new `nethook/<unix time>` directory next to the
    /// executable.
    pub fn new() -> std::io::Result<Self> {
        let exe = std::env::current_exe()?;
        let base = exe.parent().unwrap_or(Path::new("."));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let directory = base.join("nethook").join(now.to_string());
        std::fs::create_dir_all(&directory)?;
        tracing::debug!(directory = %directory.display(), "created nethook directory");
        Ok(Self { directory, message_number: AtomicU64::new(0) })
    }

    /// Logs to an existing directory.
    pub fn with_directory(directory: impl Into<PathBuf>) -> std::io::Result<Self> {
        let directory = directory.into();
        if !directory.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} does not exist", directory.display()),
            ));
        }
        Ok(Self { directory, message_number: AtomicU64::new(0) })
    }

    /// The directory messages are written to.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The file name for the next message.
    fn file_name(&self, direction: &str, emsg: EMsg) -> String {
        let number = self.message_number.fetch_add(1, Ordering::Relaxed) + 1;
        format!("{number:03}_{direction}_{}_k_EMsg{emsg}.bin", emsg.0)
    }

    fn log(&self, direction: &str, emsg: EMsg, data: &[u8]) {
        let path = self.directory.join(self.file_name(direction, emsg));
        if let Err(e) = std::fs::write(&path, data) {
            tracing::debug!(path = %path.display(), "failed to write nethook message: {e}");
        }
    }
}

impl NetworkListener for NetHookListener {
    fn on_incoming(&self, emsg: EMsg, data: &[u8]) {
        tracing::debug!(" <- Recv'd EMsg: {emsg} ({})", emsg.0);
        self.log("in", emsg, data);
    }

    fn on_outgoing(&self, emsg: EMsg, data: &[u8]) {
        tracing::debug!("Sent -> EMsg: {emsg} ({})", emsg.0);
        self.log("out", emsg, data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        let dir = std::env::temp_dir().join(format!("steamkit-nethook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = NetHookListener::with_directory(&dir).unwrap();
        listener.on_outgoing(EMsg::ClientLogon, &[1, 2, 3]);
        listener.on_incoming(EMsg(0x7FFF_0000), &[]);
        let mut names: Vec<_> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["001_out_5514_k_EMsgClientLogon.bin", "002_in_2147418112_k_EMsg2147418112.bin"]);
        assert_eq!(std::fs::read(dir.join(&names[0])).unwrap(), [1, 2, 3]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(NetHookListener::with_directory(&dir).is_err());
    }

    #[test]
    fn emsg_of_raw_messages() {
        assert_eq!(raw_emsg(&(5514u32 | 0x8000_0000).to_le_bytes()), EMsg::ClientLogon);
        assert_eq!(raw_emsg(&[]), EMsg(0));
    }
}
