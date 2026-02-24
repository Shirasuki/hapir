use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcListSkillsRequest {
    #[serde(default = "default_agent")]
    pub agent: String,
}

fn default_agent() -> String {
    "claude".into()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcSkillSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RpcListSkillsResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<RpcSkillSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
