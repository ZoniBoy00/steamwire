//! Apps, packages and licenses (SteamKit2's `SteamApps`): PICS, depot keys,
//! ownership tickets, free licenses, beta passwords.

mod callbacks;
mod requests;

pub use callbacks::*;

use std::collections::BTreeMap;

use steamkit_protos::steam::{
    c_msg_client_pics_product_info_request as req, c_msg_client_pics_product_info_response as resp,
    CMsgClientPicsAccessTokenRequest, CMsgClientPicsAccessTokenResponse, CMsgClientPicsProductInfoRequest,
    CMsgClientPicsProductInfoResponse,
};

use crate::client::SteamClient;
use crate::enums::EMsg;
use crate::error::Result;
use crate::messages::ProtoMsg;
use crate::types::kv::KeyValue;

/// An app or package to request, with its access token (0 if none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicsRequest {
    /// App or package ID.
    pub id: u32,
    /// Access token from [`SteamClient::pics_get_access_tokens`].
    pub access_token: u64,
}

impl From<u32> for PicsRequest {
    fn from(id: u32) -> Self {
        Self { id, access_token: 0 }
    }
}

/// One app or package from a PICS product info response.
#[derive(Clone, Debug)]
pub struct PicsProductInfo {
    /// App or package ID.
    pub id: u32,
    /// The change number.
    pub change_number: u32,
    /// Whether an access token was required but missing.
    pub missing_token: bool,
    /// SHA-1 of the data.
    pub sha: Vec<u8>,
    /// The parsed data; an empty node if there was none or it was invalid.
    pub key_values: KeyValue,
}

/// The combined result of a PICS product info request.
#[derive(Clone, Debug, Default)]
pub struct PicsProductInfoResult {
    /// Apps by ID.
    pub apps: BTreeMap<u32, PicsProductInfo>,
    /// Packages by ID.
    pub packages: BTreeMap<u32, PicsProductInfo>,
    /// IDs Steam doesn't know.
    pub unknown_apps: Vec<u32>,
    /// IDs Steam doesn't know.
    pub unknown_packages: Vec<u32>,
}

impl PicsProductInfoResult {
    /// Adds one (possibly partial) response.
    pub fn extend(&mut self, response: CMsgClientPicsProductInfoResponse) {
        for app in response.apps {
            let info = parse_app(app);
            self.apps.insert(info.id, info);
        }
        for package in response.packages {
            let info = parse_package(package);
            self.packages.insert(info.id, info);
        }
        self.unknown_apps.extend(response.unknown_appids);
        self.unknown_packages.extend(response.unknown_packageids);
    }
}

/// Parses a NUL terminated text KeyValues buffer.
pub(crate) fn parse_text_buffer(buffer: &[u8]) -> Option<KeyValue> {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let text = String::from_utf8_lossy(&buffer[..end]);
    if text.trim().is_empty() {
        return None;
    }
    KeyValue::parse_text(&text).ok()
}

fn empty() -> KeyValue {
    KeyValue::new_object("", Vec::new())
}

/// Parses an app: its buffer is text KeyValues, NUL terminated.
pub fn parse_app(app: resp::AppInfo) -> PicsProductInfo {
    let key_values = app.buffer.as_deref().and_then(parse_text_buffer).unwrap_or_else(empty);
    PicsProductInfo {
        id: app.appid(),
        change_number: app.change_number(),
        missing_token: app.missing_token(),
        sha: app.sha.unwrap_or_default(),
        key_values,
    }
}

/// Parses a package: its buffer is a `u32` format (1) and binary KeyValues.
pub fn parse_package(package: resp::PackageInfo) -> PicsProductInfo {
    let key_values = package
        .buffer
        .as_deref()
        .filter(|b| b.len() >= 4 && b[..4] == 1u32.to_le_bytes())
        .and_then(|b| KeyValue::parse_binary(&b[4..]).ok())
        .unwrap_or_else(empty);
    PicsProductInfo {
        id: package.packageid(),
        change_number: package.change_number(),
        missing_token: package.missing_token(),
        sha: package.sha.unwrap_or_default(),
        key_values,
    }
}

impl SteamClient {
    /// Requests app and package info, collecting every part of the response.
    ///
    /// ```no_run
    /// # async fn run(client: steamkit::SteamClient) -> steamkit::Result<()> {
    /// let info = client.pics_get_product_info(&[480.into()], &[]).await?;
    /// println!("{}", info.apps[&480].key_values["common"]["name"].as_str().unwrap_or("?"));
    /// # Ok(()) }
    /// ```
    pub async fn pics_get_product_info(
        &self,
        apps: &[PicsRequest],
        packages: &[PicsRequest],
    ) -> Result<PicsProductInfoResult> {
        let request = CMsgClientPicsProductInfoRequest {
            apps: apps
                .iter()
                .map(|a| req::AppInfo {
                    appid: Some(a.id),
                    access_token: Some(a.access_token),
                    only_public_obsolete: Some(false),
                })
                .collect(),
            packages: packages
                .iter()
                .map(|p| req::PackageInfo { packageid: Some(p.id), access_token: Some(p.access_token) })
                .collect(),
            meta_data_only: Some(false),
            ..Default::default()
        };
        let mut job = self.send_job(ProtoMsg::new(EMsg::ClientPICSProductInfoRequest, request))?;

        let mut result = PicsProductInfoResult::default();
        loop {
            let packet = job.recv().await?;
            let response: CMsgClientPicsProductInfoResponse = packet.decode()?;
            let pending = response.response_pending();
            result.extend(response);
            if !pending {
                return Ok(result);
            }
        }
    }

    /// Requests access tokens for apps and packages.
    pub async fn pics_get_access_tokens(
        &self,
        apps: &[u32],
        packages: &[u32],
    ) -> Result<CMsgClientPicsAccessTokenResponse> {
        let packet = self
            .call(ProtoMsg::new(
                EMsg::ClientPICSAccessTokenRequest,
                CMsgClientPicsAccessTokenRequest { appids: apps.to_vec(), packageids: packages.to_vec() },
            ))
            .await?;
        packet.decode()
    }
}
