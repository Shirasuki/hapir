use std::collections::HashMap;

use serde_json::Value;

use crate::types::{
    AgentMessage, PlanItem, PlanItemPriority, PlanItemStatus, ToolCallStatus, ToolResultStatus,
};
use crate::utils::derive_tool_name;

use super::constants;

fn normalize_status(status: Option<&str>) -> ToolCallStatus {
    match status {
        Some("in_progress") => ToolCallStatus::InProgress,
        Some("completed") => ToolCallStatus::Completed,
        Some("failed") => ToolCallStatus::Failed,
        _ => ToolCallStatus::Pending,
    }
}

fn extract_explicit_audience(annotations: &Value) -> Vec<String> {
    let arr = match annotations.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut audiences = Vec::new();
    for entry in arr {
        // String form: "assistant" / "user"
        if let Some(s) = entry.as_str() {
            audiences.push(s.to_string());
            continue;
        }
        // Object form: {audience: "assistant"} or {value: {audience: "assistant"}}
        if let Some(obj) = entry.as_object() {
            let inner = obj
                .get("audience")
                .or_else(|| obj.get("value").and_then(|v| v.get("audience")));
            if let Some(aud) = inner.and_then(|v| v.as_str()) {
                audiences.push(aud.to_string());
            }
        }
    }
    audiences
}

fn extract_text_content(block: &Value) -> Option<&str> {
    let obj = block.as_object()?;
    if obj.get("type")?.as_str()? != "text" {
        return None;
    }
    let audiences = extract_explicit_audience(obj.get("annotations").unwrap_or(&Value::Null));
    if !audiences.is_empty() && !audiences.iter().any(|a| a == "assistant") {
        return None;
    }
    obj.get("text")?.as_str()
}

fn normalize_plan_entries(entries: &Value) -> Vec<PlanItem> {
    let arr = match entries.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut items = Vec::new();
    for entry in arr {
        let obj = match entry.as_object() {
            Some(o) => o,
            None => continue,
        };

        let content = match obj.get("content").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };

        let priority = match obj.get("priority").and_then(|v| v.as_str()) {
            Some("high") => PlanItemPriority::High,
            Some("medium") => PlanItemPriority::Medium,
            Some("low") => PlanItemPriority::Low,
            _ => continue,
        };

        let status = match obj.get("status").and_then(|v| v.as_str()) {
            Some("pending") => PlanItemStatus::Pending,
            Some("in_progress") => PlanItemStatus::InProgress,
            Some("completed") => PlanItemStatus::Completed,
            _ => continue,
        };

        items.push(PlanItem {
            content,
            priority,
            status,
        });
    }

    items
}

pub struct AcpMessageHandler {
    tool_calls: HashMap<String, ToolCallInfo>,
    buffered_text: String,
    streaming_message_id: Option<String>,
    last_sent_text: String,
    on_message: Box<dyn Fn(AgentMessage) + Send + Sync>,
}

struct ToolCallInfo {
    name: String,
    input: Value,
}

impl AcpMessageHandler {
    pub fn new<F>(on_message: F) -> Self
    where
        F: Fn(AgentMessage) + Send + Sync + 'static,
    {
        Self {
            tool_calls: HashMap::new(),
            buffered_text: String::new(),
            streaming_message_id: None,
            last_sent_text: String::new(),
            on_message: Box::new(on_message),
        }
    }

    pub fn flush_text(&mut self) {
        if self.buffered_text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.buffered_text);
        (self.on_message)(AgentMessage::Text { text });
    }

    fn send_text_delta(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if self.streaming_message_id.is_none() {
            self.streaming_message_id = Some(uuid::Uuid::new_v4().to_string());
            self.last_sent_text.clear();
            self.buffered_text.clear();
        }

        let message_id = self.streaming_message_id.as_ref().unwrap().clone();

        let is_cumulative = text.starts_with(&self.last_sent_text);
        let delta_text = if is_cumulative {
            &text[self.last_sent_text.len()..]
        } else {
            text
        };

        if !delta_text.is_empty() {
            (self.on_message)(AgentMessage::TextDelta {
                message_id,
                text: delta_text.to_string(),
                is_final: false,
            });
            self.last_sent_text = text.to_string();
        }

        // 累积完整文本：累积格式直接替换，增量格式追加
        if is_cumulative {
            self.buffered_text = text.to_string();
        } else {
            self.buffered_text.push_str(delta_text);
        }
    }

    pub fn finalize_stream(&mut self) {
        if let Some(message_id) = self.streaming_message_id.take() {
            (self.on_message)(AgentMessage::TextDelta {
                message_id,
                text: String::new(),
                is_final: true,
            });
            self.last_sent_text.clear();
            // buffered_text 保留，供 flush_text 发送完整消息
        }
    }

    pub fn handle_update(&mut self, update: &Value) {
        let obj = match update.as_object() {
            Some(o) => o,
            None => return,
        };

        let update_type = match obj.get("sessionUpdate").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return,
        };

        match update_type {
            constants::AGENT_MESSAGE_CHUNK => {
                if let Some(content) = obj.get("content")
                    && let Some(text) = extract_text_content(content)
                {
                    self.send_text_delta(text);
                }
            }
            constants::AGENT_THOUGHT_CHUNK => {}
            constants::TOOL_CALL => {
                self.finalize_stream();
                self.flush_text();
                self.handle_tool_call(update);
            }
            constants::TOOL_CALL_UPDATE => {
                self.finalize_stream();
                self.flush_text();
                self.handle_tool_call_update(update);
            }
            constants::PLAN => {
                self.finalize_stream();
                self.flush_text();
                if let Some(entries) = obj.get("entries") {
                    let items = normalize_plan_entries(entries);
                    if !items.is_empty() {
                        (self.on_message)(AgentMessage::Plan { items });
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_tool_call(&mut self, update: &Value) {
        let tool_call_id = match update.get("toolCallId").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return,
        };

        let name = derive_tool_name(
            update.get("title").and_then(|v| v.as_str()),
            update.get("kind").and_then(|v| v.as_str()),
            update.get("rawInput"),
        );
        let input = update.get("rawInput").cloned().unwrap_or(Value::Null);
        let status = normalize_status(update.get("status").and_then(|v| v.as_str()));

        self.tool_calls.insert(
            tool_call_id.clone(),
            ToolCallInfo {
                name: name.clone(),
                input: input.clone(),
            },
        );

        (self.on_message)(AgentMessage::ToolCall {
            id: tool_call_id,
            name,
            input,
            status,
        });
    }

    fn handle_tool_call_update(&mut self, update: &Value) {
        let tool_call_id = match update.get("toolCallId").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return,
        };

        let status = normalize_status(update.get("status").and_then(|v| v.as_str()));

        if update.get("rawInput").is_some() {
            let name = derive_tool_name(
                update.get("title").and_then(|v| v.as_str()),
                update.get("kind").and_then(|v| v.as_str()),
                update.get("rawInput"),
            );
            let input = update.get("rawInput").cloned().unwrap_or(Value::Null);
            self.tool_calls.insert(
                tool_call_id.clone(),
                ToolCallInfo {
                    name: name.clone(),
                    input: input.clone(),
                },
            );
            (self.on_message)(AgentMessage::ToolCall {
                id: tool_call_id.clone(),
                name,
                input,
                status,
            });
        } else if let Some(existing) = self.tool_calls.get(&tool_call_id)
            && (status == ToolCallStatus::InProgress || status == ToolCallStatus::Pending)
        {
            (self.on_message)(AgentMessage::ToolCall {
                id: tool_call_id.clone(),
                name: existing.name.clone(),
                input: existing.input.clone(),
                status,
            });
        }

        if status == ToolCallStatus::Completed || status == ToolCallStatus::Failed {
            let output = update
                .get("rawOutput")
                .or_else(|| update.get("content"))
                .cloned()
                .unwrap_or(Value::Null);

            let result_status = if status == ToolCallStatus::Failed {
                ToolResultStatus::Failed
            } else {
                ToolResultStatus::Completed
            };

            (self.on_message)(AgentMessage::ToolResult {
                id: tool_call_id,
                output,
                status: result_status,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    fn make_handler() -> (AcpMessageHandler, Arc<Mutex<Vec<AgentMessage>>>) {
        let msgs: Arc<Mutex<Vec<AgentMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let msgs_clone = Arc::clone(&msgs);
        let handler = AcpMessageHandler::new(move |m| {
            msgs_clone.lock().unwrap().push(m);
        });
        (handler, msgs)
    }

    #[test]
    fn tool_completes_without_payload_has_null_output() {
        let (mut handler, msgs) = make_handler();

        // First register the tool call so the update path proceeds
        let call = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc1",
            "title": "bash",
            "status": "in_progress"
        });
        handler.handle_update(&call);

        // Now send a completion with no rawOutput / content
        let update = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc1",
            "status": "completed"
        });
        handler.handle_update(&update);

        let collected = msgs.lock().unwrap();
        let result = collected.iter().find_map(|m| match m {
            AgentMessage::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        });
        assert_eq!(result, Some(Value::Null));
    }

    #[test]
    fn text_block_without_annotations_is_included() {
        let block = json!({"type": "text", "text": "hello"});
        assert_eq!(extract_text_content(&block), Some("hello"));
    }

    #[test]
    fn text_block_with_assistant_audience_is_included() {
        let block = json!({"type": "text", "text": "hi", "annotations": ["assistant"]});
        assert_eq!(extract_text_content(&block), Some("hi"));
    }

    #[test]
    fn text_block_with_user_only_audience_is_excluded() {
        let block = json!({"type": "text", "text": "hi", "annotations": ["user"]});
        assert_eq!(extract_text_content(&block), None);
    }
}
