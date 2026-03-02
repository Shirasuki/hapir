use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum FsEntryType {
    File,
    Directory,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcListDirectoryRequest {
    #[serde(default = "default_dot")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcGetDirectoryTreeRequest {
    #[serde(default = "default_dot")]
    pub path: String,
    #[serde(default = "default_max_depth")]
    pub max_depth: i64,
}

fn default_dot() -> String {
    ".".into()
}

fn default_max_depth() -> i64 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RpcDirectoryEntry {
    pub name: String,
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub entry_type: FsEntryType,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub modified: Option<u64>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RpcListDirectoryResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<RpcDirectoryEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RpcTreeNode {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub node_type: FsEntryType,
    #[ts(type = "number")]
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub modified: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<RpcTreeNode>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RpcGetDirectoryTreeResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree: Option<RpcTreeNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
