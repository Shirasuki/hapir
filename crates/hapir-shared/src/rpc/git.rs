use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcGitStatusRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcGitDiffNumstatRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcGitDiffFileRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}
