use serde::{Deserialize, Serialize};
use serde_json::Value;

// -- FlatMessage: used by opencode/gemini forward_agent_message + error messages --

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantTextInner {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallPayload {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub status: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResultPayload {
    pub id: String,
    pub output: Value,
    pub status: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum FlatMessage {
    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(rename = "exitReason", skip_serializing_if = "Option::is_none")]
        exit_reason: Option<String>,
    },
    #[serde(rename = "assistant")]
    AssistantText { message: AssistantTextInner },
    #[serde(rename = "tool_call")]
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: ToolCallPayload,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(rename = "toolResult")]
        tool_result: ToolResultPayload,
    },
    #[serde(rename = "plan")]
    Plan { entries: Vec<Value> },
}

// -- RoleWrappedMessage: used by claude output / codex / user prompt --

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageMeta {
    #[serde(rename = "sentFrom")]
    pub sent_from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeMessageBody {
    pub role: String,
    pub content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeOutputData {
    #[serde(rename = "assistant")]
    Assistant {
        message: ClaudeMessageBody,
        #[serde(rename = "parentUuid", skip_serializing_if = "Option::is_none")]
        parent_uuid: Option<String>,
    },
    #[serde(rename = "user")]
    User {
        message: ClaudeMessageBody,
        #[serde(rename = "parentUuid", skip_serializing_if = "Option::is_none")]
        parent_uuid: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum AgentContent {
    #[serde(rename = "output")]
    Output { data: ClaudeOutputData },
    #[serde(rename = "codex")]
    Codex { data: Value },
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleWrappedMessage {
    pub role: String,
    pub content: AgentContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<MessageMeta>,
}

// -- SessionMessage: top-level untagged enum --

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SessionMessage {
    RoleWrapped(RoleWrappedMessage),
    Flat(FlatMessage),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flat_error_matches_json() {
        let msg = FlatMessage::Error {
            message: "something broke".into(),
            exit_reason: None,
        };
        let expected = json!({"type": "error", "message": "something broke"});
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
        let rt: FlatMessage = serde_json::from_value(expected).unwrap();
        assert_eq!(rt, msg);
    }

    #[test]
    fn flat_error_with_exit_reason() {
        let msg = FlatMessage::Error {
            message: "fail".into(),
            exit_reason: Some("spawn_error".into()),
        };
        let expected = json!({"type": "error", "message": "fail", "exitReason": "spawn_error"});
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn flat_assistant_text_matches_json() {
        let msg = FlatMessage::AssistantText {
            message: AssistantTextInner {
                role: "assistant".into(),
                content: "hello".into(),
            },
        };
        let expected = json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": "hello"}
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
        let rt: FlatMessage = serde_json::from_value(expected).unwrap();
        assert_eq!(rt, msg);
    }

    #[test]
    fn flat_tool_call_matches_json() {
        let msg = FlatMessage::ToolCall {
            tool_call: ToolCallPayload {
                id: "tc1".into(),
                name: "bash".into(),
                input: json!({"cmd": "ls"}),
                status: json!("running"),
            },
        };
        let expected = json!({
            "type": "tool_call",
            "toolCall": {"id": "tc1", "name": "bash", "input": {"cmd": "ls"}, "status": "running"}
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn flat_tool_result_matches_json() {
        let msg = FlatMessage::ToolResult {
            tool_result: ToolResultPayload {
                id: "tc1".into(),
                output: json!("ok"),
                status: json!("done"),
            },
        };
        let expected = json!({
            "type": "tool_result",
            "toolResult": {"id": "tc1", "output": "ok", "status": "done"}
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn flat_plan_matches_json() {
        let msg = FlatMessage::Plan {
            entries: vec![json!({"step": 1})],
        };
        let expected = json!({"type": "plan", "entries": [{"step": 1}]});
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn role_wrapped_claude_output_assistant() {
        let msg = RoleWrappedMessage {
            role: "assistant".into(),
            content: AgentContent::Output {
                data: ClaudeOutputData::Assistant {
                    message: ClaudeMessageBody {
                        role: "assistant".into(),
                        content: json!([{"type": "text", "text": "hi"}]),
                        usage: None,
                    },
                    parent_uuid: None,
                },
            },
            meta: None,
        };
        let expected = json!({
            "role": "assistant",
            "content": {
                "type": "output",
                "data": {
                    "type": "assistant",
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "text", "text": "hi"}]
                    }
                }
            }
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
        let rt: RoleWrappedMessage = serde_json::from_value(expected).unwrap();
        assert_eq!(rt, msg);
    }

    #[test]
    fn role_wrapped_claude_output_user() {
        let msg = RoleWrappedMessage {
            role: "assistant".into(),
            content: AgentContent::Output {
                data: ClaudeOutputData::User {
                    message: ClaudeMessageBody {
                        role: "user".into(),
                        content: json!([{"type": "tool_result"}]),
                        usage: None,
                    },
                    parent_uuid: Some("abc".into()),
                },
            },
            meta: None,
        };
        let expected = json!({
            "role": "assistant",
            "content": {
                "type": "output",
                "data": {
                    "type": "user",
                    "parentUuid": "abc",
                    "message": {
                        "role": "user",
                        "content": [{"type": "tool_result"}]
                    }
                }
            }
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn role_wrapped_codex() {
        let msg = RoleWrappedMessage {
            role: "assistant".into(),
            content: AgentContent::Codex {
                data: json!({"type": "message", "message": "hi"}),
            },
            meta: None,
        };
        let expected = json!({
            "role": "assistant",
            "content": {"type": "codex", "data": {"type": "message", "message": "hi"}}
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn role_wrapped_user_text_with_meta() {
        let msg = RoleWrappedMessage {
            role: "user".into(),
            content: AgentContent::Text {
                text: "hello".into(),
            },
            meta: Some(MessageMeta {
                sent_from: "cli".into(),
            }),
        };
        let expected = json!({
            "role": "user",
            "content": {"type": "text", "text": "hello"},
            "meta": {"sentFrom": "cli"}
        });
        assert_eq!(serde_json::to_value(&msg).unwrap(), expected);
    }

    #[test]
    fn session_message_untagged_roundtrip() {
        let flat = SessionMessage::Flat(FlatMessage::Error {
            message: "err".into(),
            exit_reason: None,
        });
        let json = serde_json::to_value(&flat).unwrap();
        assert_eq!(json, json!({"type": "error", "message": "err"}));

        let wrapped = SessionMessage::RoleWrapped(RoleWrappedMessage {
            role: "assistant".into(),
            content: AgentContent::Codex {
                data: json!({"type": "message", "message": "x"}),
            },
            meta: None,
        });
        let json2 = serde_json::to_value(&wrapped).unwrap();
        assert!(json2.get("role").is_some());
    }
}
