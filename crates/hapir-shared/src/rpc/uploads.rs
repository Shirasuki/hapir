use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcUploadFileRequest {
    pub filename: String,
    pub content: String,
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcDeleteUploadRequest {
    pub path: String,
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcUploadFileResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcDeleteUploadResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
