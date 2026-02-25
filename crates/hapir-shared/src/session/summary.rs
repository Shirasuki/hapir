use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::modes::ModelMode;
use crate::schemas::{Session, SessionWorktreeMetadata, TodoStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[ts(rename_all = "camelCase")]
pub struct SessionSummaryMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flavor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<SessionWorktreeMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[ts(export)]
pub struct SummaryText {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[ts(rename_all = "camelCase")]
pub struct TodoProgress {
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[ts(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub active: bool,
    pub thinking: bool,
    pub active_at: f64,
    pub updated_at: f64,
    pub metadata: Option<SessionSummaryMetadata>,
    pub todo_progress: Option<TodoProgress>,
    pub pending_requests_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_mode: Option<ModelMode>,
}

pub fn to_session_summary(session: &Session) -> SessionSummary {
    let pending_requests_count = session
        .agent_state
        .as_ref()
        .and_then(|s| s.requests.as_ref())
        .map(|r| r.len())
        .unwrap_or(0);

    let metadata = session.metadata.as_ref().map(|m| SessionSummaryMetadata {
        name: m.name.clone(),
        path: m.path.clone(),
        machine_id: m.machine_id.clone(),
        summary: m.summary.as_ref().map(|s| SummaryText {
            text: s.text.clone(),
        }),
        flavor: m.flavor.clone(),
        worktree: m.worktree.clone(),
    });

    let todo_progress = session.todos.as_ref().and_then(|todos| {
        if todos.is_empty() {
            None
        } else {
            Some(TodoProgress {
                completed: todos
                    .iter()
                    .filter(|t| t.status == TodoStatus::Completed)
                    .count(),
                total: todos.len(),
            })
        }
    });

    SessionSummary {
        id: session.id.clone(),
        active: session.active,
        thinking: session.thinking,
        active_at: session.active_at,
        updated_at: session.updated_at,
        metadata,
        todo_progress,
        pending_requests_count,
        model_mode: session.model_mode,
    }
}
