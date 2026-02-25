use hapir_acp::types::AgentMessage;
use hapir_shared::session::{
    AssistantTextInner, FlatMessage, ToolCallPayload, ToolResultPayload,
};

/// Convert an ACP AgentMessage to a FlatMessage for opencode/gemini forwarding.
/// Returns None for TextDelta, TurnComplete, and ThinkingStatus (handled separately).
pub fn agent_message_to_flat(msg: &AgentMessage) -> Option<FlatMessage> {
    match msg {
        AgentMessage::Text { text } => Some(FlatMessage::AssistantText {
            message: AssistantTextInner {
                role: "assistant".into(),
                content: text.clone(),
            },
        }),
        AgentMessage::ToolCall {
            id,
            name,
            input,
            status,
        } => Some(FlatMessage::ToolCall {
            tool_call: ToolCallPayload {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                status: serde_json::to_value(status).unwrap_or_default(),
            },
        }),
        AgentMessage::ToolResult { id, output, status } => Some(FlatMessage::ToolResult {
            tool_result: ToolResultPayload {
                id: id.clone(),
                output: output.clone(),
                status: serde_json::to_value(status).unwrap_or_default(),
            },
        }),
        AgentMessage::Plan { items } => Some(FlatMessage::Plan {
            entries: items.iter().map(|i| serde_json::to_value(i).unwrap_or_default()).collect(),
        }),
        AgentMessage::Error { message } => Some(FlatMessage::AssistantText {
            message: AssistantTextInner {
                role: "assistant".into(),
                content: message.clone(),
            },
        }),
        AgentMessage::TextDelta { .. }
        | AgentMessage::TurnComplete { .. }
        | AgentMessage::ThinkingStatus { .. } => None,
    }
}
