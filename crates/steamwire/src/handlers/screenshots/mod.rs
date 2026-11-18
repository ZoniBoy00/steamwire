//! Screenshots (SteamKit2's `SteamScreenshots`).

use steamwire_protos::steam::{CMsgClientUcmAddScreenshot, CMsgClientUcmAddScreenshotResponse};

use crate::client::SteamClient;
use crate::enums::{EMsg, EResult, EUCMFilePrivacyState};
use crate::error::{Error, Result};
use crate::handlers::proto_callbacks;
use crate::messages::{Packet, ProtoMsg};
use crate::types::GameId;

/// The width of screenshot thumbnails.
pub const SCREENSHOT_THUMBNAIL_WIDTH: u32 = 200;

proto_callbacks! {
    /// Messages handled by SteamKit2's `SteamScreenshots`.
    pub enum ScreenshotsCallback {
        /// `ClientUCMAddScreenshotResponse`.
        ScreenshotAdded(CMsgClientUcmAddScreenshotResponse) = ClientUCMAddScreenshotResponse,
    }
}

impl ScreenshotsCallback {
    /// Decodes a packet handled by this module.
    pub fn from_packet(packet: &Packet) -> Option<Self> {
        Self::decode_proto(packet)
    }
}

/// A screenshot already uploaded to UFS.
#[derive(Clone, Debug, Default)]
pub struct ScreenshotDetails {
    /// The game.
    pub game_id: Option<GameId>,
    /// The UFS path of the image.
    pub ufs_image_file_path: String,
    /// The UFS path of the thumbnail.
    pub ufs_thumbnail_file_path: String,
    /// The caption.
    pub caption: String,
    /// Who can see it.
    pub privacy: EUCMFilePrivacyState,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Unix time it was taken.
    pub creation_time: u32,
    /// Whether it contains spoilers.
    pub contains_spoilers: bool,
}

impl SteamClient {
    /// Adds a screenshot to the library and returns its ID.
    pub async fn add_screenshot(&self, details: &ScreenshotDetails) -> Result<u64> {
        let response: CMsgClientUcmAddScreenshotResponse = self
            .call(ProtoMsg::new(
                EMsg::ClientUCMAddScreenshot,
                CMsgClientUcmAddScreenshot {
                    appid: details.game_id.map(|g| g.app_id()),
                    caption: Some(details.caption.clone()),
                    filename: Some(details.ufs_image_file_path.clone()),
                    permissions: Some(details.privacy.0 as u32),
                    thumbname: Some(details.ufs_thumbnail_file_path.clone()),
                    width: Some(details.width),
                    height: Some(details.height),
                    rtime32_created: Some(details.creation_time),
                    spoiler_tag: Some(details.contains_spoilers),
                    ..Default::default()
                },
            ))
            .await?
            .decode()?;
        match EResult(response.eresult()) {
            EResult::OK => Ok(response.screenshotid()),
            other => Err(Error::Steam(other)),
        }
    }
}
