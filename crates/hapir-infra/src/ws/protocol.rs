use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct WsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub event: String,
    pub data: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsMessage {
    #[serde(default)]
    pub id: Option<String>,
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

impl WsRequest {
    pub fn with_ack(event: impl Into<String>, data: Value) -> (Self, String) {
        let id = Uuid::new_v4().to_string();
        let req = Self {
            id: Some(id.clone()),
            event: event.into(),
            data,
        };
        (req, id)
    }

    pub fn fire(event: impl Into<String>, data: Value) -> Self {
        Self {
            id: None,
            event: event.into(),
            data,
        }
    }
}
