use std::collections::HashMap;

use serde::Serialize;

use crate::common::session::Session;
use crate::common::summary::SessionSummary;

// sessions
#[derive(Serialize)]
pub struct SessionsData {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Serialize)]
pub struct SessionData {
    pub session: Session,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeData {
    pub session_id: String,
}

// machines
#[derive(Serialize)]
pub struct MachinesData {
    pub machines: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub struct PathsExistsData {
    pub exists: HashMap<String, bool>,
}

// auth / telegram_bind
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthData {
    pub token: String,
    pub user: AuthUser,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUser {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

// push
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VapidKeyData {
    pub public_key: String,
}

// session_workspace
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchData {
    pub files: Vec<FileSearchItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchItem {
    pub file_name: String,
    pub file_path: String,
    pub full_path: String,
    pub file_type: String,
}

// voice
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTokenData {
    pub token: String,
    pub agent_id: String,
}

// cli — get_machine
#[derive(Serialize)]
pub struct MachineData {
    pub machine: serde_json::Value,
}

// health
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthData {
    pub status: &'static str,
    pub protocol_version: u32,
}
