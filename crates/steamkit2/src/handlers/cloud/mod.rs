//! Steam Cloud (SteamKit2's `SteamCloud`), over the `Cloud` service.

use steamkit2_protos::steam::{
    CCloudGetFileDetailsRequest, CCloudGetFileDetailsResponse, CCloudGetSingleFileInfoRequest,
    CCloudGetSingleFileInfoResponse, CCloudShareFileRequest, CCloudShareFileResponse,
};

use crate::client::SteamClient;
use crate::error::Result;
use crate::types::UgcHandle;

impl SteamClient {
    /// Requests the details of a UGC file (`Cloud.GetFileDetails#1`).
    pub async fn request_ugc_details(&self, ugc_id: UgcHandle) -> Result<CCloudGetFileDetailsResponse> {
        self.call_service(
            "Cloud.GetFileDetails#1",
            CCloudGetFileDetailsRequest { ugcid: Some(ugc_id.0 .0), appid: None },
        )
        .await
    }

    /// Requests the details of a Cloud file (`Cloud.GetSingleFileInfo#1`).
    pub async fn get_single_file_info(&self, app_id: u32, file_name: &str) -> Result<CCloudGetSingleFileInfoResponse> {
        self.call_service(
            "Cloud.GetSingleFileInfo#1",
            CCloudGetSingleFileInfoRequest { app_id: Some(app_id), file_name: Some(file_name.to_string()) },
        )
        .await
    }

    /// Shares a Cloud file, returning its UGC handle (`Cloud.ShareFile#1`).
    pub async fn share_file(&self, app_id: u32, file_name: &str) -> Result<UgcHandle> {
        let response: CCloudShareFileResponse = self
            .call_service(
                "Cloud.ShareFile#1",
                CCloudShareFileRequest { app_id: Some(app_id), file_name: Some(file_name.to_string()) },
            )
            .await?;
        Ok(UgcHandle::from(response.hcontent()))
    }
}
