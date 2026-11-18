use tokio::sync::mpsc;

use std::time::Duration;

use super::SteamClient;
use crate::enums::EMsg;
use crate::error::{Error, Result};
use crate::messages::Packet;
use crate::types::JobId;

/// The replies to a job started with [`SteamClient::send_job`].
#[derive(Debug)]
pub struct JobReceiver {
    pub(super) job: JobId,
    pub(super) rx: mpsc::UnboundedReceiver<Packet>,
    pub(super) timeout: Duration,
    pub(super) client: SteamClient,
}

impl JobReceiver {
    /// The job ID.
    pub fn job_id(&self) -> JobId {
        self.job
    }

    /// How long [`recv`](Self::recv) waits; [`Config::job_timeout`](crate::Config::job_timeout)
    /// unless changed with [`set_timeout`](Self::set_timeout).
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Changes how long [`recv`](Self::recv) waits (SteamKit2's
    /// `AsyncJob.Timeout`).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// The next reply. Fails with [`Error::Timeout`] after
    /// [`timeout`](Self::timeout) and [`Error::NotConnected`] on disconnect.
    ///
    /// `JobHeartbeat` messages restart the timeout and `DestJobFailed` fails
    /// the job with [`Error::JobFailed`].
    pub async fn recv(&mut self) -> Result<Packet> {
        loop {
            match tokio::time::timeout(self.timeout, self.rx.recv()).await {
                Ok(Some(packet)) => match packet.emsg {
                    EMsg::JobHeartbeat => continue,
                    EMsg::DestJobFailed => return Err(Error::JobFailed),
                    _ => return Ok(packet),
                },
                Ok(None) => return Err(Error::NotConnected),
                Err(_) => return Err(Error::Timeout),
            }
        }
    }
}

impl Drop for JobReceiver {
    fn drop(&mut self) {
        self.client.inner.jobs.lock().unwrap().remove(&self.job.0);
    }
}
