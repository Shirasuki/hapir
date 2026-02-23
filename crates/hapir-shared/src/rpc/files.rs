use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcReadFileRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcWriteFileRequest {
    pub path: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcReadFileResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcWriteFileResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
