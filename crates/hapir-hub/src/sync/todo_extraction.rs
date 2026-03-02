use hapir_shared::common::modes::AgentFlavor;
use hapir_shared::common::session_messages::unwrap_role_wrapped_record_envelope;
use hapir_shared::common::todo::{TodoItem, TodoPriority, TodoStatus};
use hapir_shared::common::utils::is_object;
use serde_json::Value;
use tracing::warn;

/// Extract TodoWrite todos from a message content envelope.
/// Dispatches to the correct parser based on agent flavor.
pub fn extract_todos_from_message_content(
    message_content: &Value,
    flavor: Option<AgentFlavor>,
) -> Option<Vec<TodoItem>> {
    let record = unwrap_role_wrapped_record_envelope(message_content)?;

    if record.role != "agent" && record.role != "assistant" {
        return None;
    }

    let content = &record.content;
    if !is_object(content) {
        return None;
    }
    let content_type = content.get("type").and_then(|v| v.as_str())?;

    match flavor {
        Some(AgentFlavor::Claude) => extract_from_claude_output(content, content_type),
        Some(AgentFlavor::Codex) => extract_from_codex_message(content, content_type)
            .or_else(|| extract_from_acp_plan(content, content_type)),
        _ => {
            warn!("extract_todos called without flavor, falling back to brute-force");
            extract_from_claude_output(content, content_type)
                .or_else(|| extract_from_codex_message(content, content_type))
                .or_else(|| extract_from_acp_plan(content, content_type))
        }
    }
}

fn extract_from_claude_output(content: &Value, content_type: &str) -> Option<Vec<TodoItem>> {
    if content_type != "output" {
        return None;
    }

    let data = content.get("data")?;
    if !is_object(data) || data.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }

    let message = data.get("message")?;
    if !is_object(message) {
        return None;
    }

    let model_content = message.get("content")?.as_array()?;

    for block in model_content {
        if !is_object(block) {
            continue;
        }
        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
            continue;
        }
        if block.get("name").and_then(|v| v.as_str()) != Some("TodoWrite") {
            continue;
        }
        let input = block.get("input")?;
        if !is_object(input) {
            continue;
        }
        if let Some(todos) = input.get("todos")
            && let Ok(items) = serde_json::from_value::<Vec<TodoItem>>(todos.clone())
        {
            return Some(items);
        }
    }

    None
}

fn extract_from_codex_message(content: &Value, content_type: &str) -> Option<Vec<TodoItem>> {
    if content_type != "codex" {
        return None;
    }

    let data = content.get("data")?;
    if !is_object(data) || data.get("type").and_then(|v| v.as_str()) != Some("tool-call") {
        return None;
    }
    if data.get("name").and_then(|v| v.as_str()) != Some("TodoWrite") {
        return None;
    }

    let input = data.get("input")?;
    if !is_object(input) {
        return None;
    }

    let todos = input.get("todos")?;
    serde_json::from_value::<Vec<TodoItem>>(todos.clone()).ok()
}

fn extract_from_acp_plan(content: &Value, content_type: &str) -> Option<Vec<TodoItem>> {
    if content_type != "codex" {
        return None;
    }

    let data = content.get("data")?;
    if !is_object(data) || data.get("type").and_then(|v| v.as_str()) != Some("plan") {
        return None;
    }

    let entries = data.get("entries")?.as_array()?;
    let mut todos = Vec::new();

    for (index, entry) in entries.iter().enumerate() {
        if !is_object(entry) {
            continue;
        }
        let content_val = entry.get("content").and_then(|v| v.as_str())?;
        let priority_str = entry.get("priority").and_then(|v| v.as_str())?;
        let status_str = entry.get("status").and_then(|v| v.as_str())?;

        let priority = match priority_str {
            "high" => TodoPriority::High,
            "medium" => TodoPriority::Medium,
            "low" => TodoPriority::Low,
            _ => continue,
        };
        let status = match status_str {
            "pending" => TodoStatus::Pending,
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            _ => continue,
        };

        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("plan-{}", index + 1));

        todos.push(TodoItem {
            content: content_val.to_string(),
            priority,
            status,
            id,
        });
    }

    if todos.is_empty() {
        return None;
    }

    Some(todos)
}
