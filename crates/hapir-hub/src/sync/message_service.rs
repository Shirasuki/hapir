use hapir_shared::schemas::{AttachmentMetadata, DecryptedMessage, SyncEvent};
use serde_json::Value;

use super::event_publisher::EventPublisher;
use super::todos::extract_todos_from_message_content;
use crate::store::Store;
use crate::store::types::StoredMessage;

/// Pagination info for message queries.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub limit: i64,
    pub before_seq: Option<i64>,
    pub next_before_seq: Option<i64>,
    pub has_more: bool,
}

/// Result of a paginated message query.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MessagesPageResult {
    pub messages: Vec<DecryptedMessage>,
    pub page: MessagePage,
}

pub struct MessageService;

impl MessageService {
    pub fn get_messages_page(
        store: &Store,
        session_id: &str,
        limit: i64,
        before_seq: Option<i64>,
    ) -> MessagesPageResult {
        use crate::store::messages;

        let stored = messages::get_messages(&store.conn(), session_id, limit, before_seq);
        let msgs: Vec<DecryptedMessage> = stored
            .iter()
            .map(|m| DecryptedMessage {
                id: m.id.clone(),
                seq: Some(m.seq as f64),
                local_id: m.local_id.clone(),
                content: m.content.clone().unwrap_or(Value::Null),
                created_at: m.created_at as f64,
            })
            .collect();

        let oldest_seq = msgs.iter().filter_map(|m| m.seq.map(|s| s as i64)).min();

        let has_more = oldest_seq
            .map(|seq| !messages::get_messages(&store.conn(), session_id, 1, Some(seq)).is_empty())
            .unwrap_or(false);

        MessagesPageResult {
            messages: msgs,
            page: MessagePage {
                limit,
                before_seq,
                next_before_seq: oldest_seq,
                has_more,
            },
        }
    }

    pub fn get_messages_after(
        store: &Store,
        session_id: &str,
        after_seq: i64,
        limit: i64,
    ) -> Vec<DecryptedMessage> {
        use crate::store::messages;

        messages::get_messages_after(&store.conn(), session_id, after_seq, limit)
            .iter()
            .map(|m| DecryptedMessage {
                id: m.id.clone(),
                seq: Some(m.seq as f64),
                local_id: m.local_id.clone(),
                content: m.content.clone().unwrap_or(Value::Null),
                created_at: m.created_at as f64,
            })
            .collect()
    }

    pub fn send_message(
        store: &Store,
        publisher: &EventPublisher,
        session_id: &str,
        namespace: &str,
        text: &str,
        local_id: Option<&str>,
        attachments: Option<&[AttachmentMetadata]>,
        sent_from: Option<&str>,
    ) -> anyhow::Result<()> {
        use crate::store::messages;

        let sent_from = sent_from.unwrap_or("webapp");

        let content = serde_json::json!({
            "role": "user",
            "content": {
                "type": "text",
                "text": text,
                "attachments": attachments,
            },
            "meta": {
                "sentFrom": sent_from,
            }
        });

        let msg = messages::add_message(&store.conn(), session_id, &content, local_id)?;

        let decrypted = DecryptedMessage {
            id: msg.id.clone(),
            seq: Some(msg.seq as f64),
            local_id: msg.local_id.clone(),
            content: msg.content.clone().unwrap_or(Value::Null),
            created_at: msg.created_at as f64,
        };

        publisher.emit(SyncEvent::MessageReceived {
            session_id: session_id.to_string(),
            namespace: Some(namespace.to_string()),
            message: decrypted,
        });

        Ok(())
    }

    /// Add a message from CLI. The CLI already has the full content, so no RPC is needed.
    /// Handles todo extraction and emits appropriate events.
    pub fn add_cli_message(
        store: &Store,
        publisher: &EventPublisher,
        session_id: &str,
        namespace: &str,
        content: &Value,
        local_id: Option<&str>,
    ) -> anyhow::Result<StoredMessage> {
        use crate::store::{messages, sessions};

        let msg = messages::add_message(&store.conn(), session_id, content, local_id)?;

        // Extract and persist todos if present
        if let Some(todos) = extract_todos_from_message_content(content)
            && let Ok(todos_val) = serde_json::to_value(&todos)
            && sessions::set_session_todos(
                &store.conn(),
                session_id,
                Some(&todos_val),
                msg.created_at,
                namespace,
            )
        {
            publisher.emit(SyncEvent::SessionUpdated {
                session_id: session_id.to_string(),
                namespace: Some(namespace.to_string()),
                data: Some(serde_json::json!({"sid": session_id})),
            });
        }

        publisher.emit(SyncEvent::MessageReceived {
            session_id: session_id.to_string(),
            namespace: Some(namespace.to_string()),
            message: DecryptedMessage {
                id: msg.id.clone(),
                seq: Some(msg.seq as f64),
                local_id: msg.local_id.clone(),
                content: msg.content.clone().unwrap_or(Value::Null),
                created_at: msg.created_at as f64,
            },
        });

        Ok(msg)
    }
}
