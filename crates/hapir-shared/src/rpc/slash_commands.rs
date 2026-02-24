use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcListSlashCommandsRequest {
    #[serde(default = "default_agent")]
    pub agent: String,
}

fn default_agent() -> String {
    "claude".into()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSlashCommand {
    pub name: String,
    pub description: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcListSlashCommandsResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<RpcSlashCommand>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
