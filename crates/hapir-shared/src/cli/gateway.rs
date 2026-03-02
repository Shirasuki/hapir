use serde::{Deserialize, Serialize};

use crate::common::agent_state::AnswersFormat;
use crate::common::message::AttachmentMetadata;
use crate::common::modes::{ModelMode, PermissionMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcPermissionRequest {
    pub id: String,
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answers: Option<AnswersFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcAbortRequest {
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcSwitchRequest {
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSetSessionConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_mode: Option<ModelMode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcUserMessageRequest<'a> {
    pub message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<&'a [AttachmentMetadata]>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSpawnSessionRequest<'a> {
    #[serde(rename = "type")]
    pub spawn_type: &'a str,
    pub directory: &'a str,
    pub agent: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcPathExistsRequest<'a> {
    pub paths: &'a [String],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcCommonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RpcCommonResponse {
    pub fn success() -> serde_json::Value {
        serde_json::to_value(Self {
            ok: true,
            reason: None,
        })
        .unwrap()
    }

    pub fn error(reason: impl Into<String>) -> serde_json::Value {
        serde_json::to_value(Self {
            ok: false,
            reason: Some(reason.into()),
        })
        .unwrap()
    }
}
