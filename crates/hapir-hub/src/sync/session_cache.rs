use super::alive_time::clamp_alive_time;
use super::event_publisher::EventPublisher;
use super::todo_extraction::extract_todos_from_message_content;
use crate::store::Store;
use crate::store::types::VersionedUpdateResult;
use anyhow::anyhow;
use hapir_shared::cli::socket::SocketErrorReason;
use hapir_shared::common::modes::{ModelMode, PermissionMode};
use hapir_shared::common::session::Session;
use hapir_shared::common::sync_event::SyncEvent;
use hapir_shared::common::utils::now_millis;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use tracing::{debug, info};

/// Max messages to scan when backfilling todos from history.
/// Override via `HAPIR_TODO_BACKFILL_LIMIT` env var.
static TODO_BACKFILL_LIMIT: LazyLock<i64> = LazyLock::new(|| {
    std::env::var("HAPIR_TODO_BACKFILL_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200)
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAccessError {
    NotFound,
    AccessDenied,
}

impl From<SessionAccessError> for SocketErrorReason {
    fn from(e: SessionAccessError) -> Self {
        match e {
            SessionAccessError::NotFound => SocketErrorReason::NotFound,
            SessionAccessError::AccessDenied => SocketErrorReason::AccessDenied,
        }
    }
}

/// Sessions are considered inactive after 30s without a heartbeat.
const SESSION_TIMEOUT_MS: i64 = 30_000;

pub struct SessionCache {
    sessions: HashMap<String, Session>,
    last_broadcast_at: HashMap<String, i64>,
    todo_backfill_attempted: HashSet<String>,
}

impl SessionCache {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            last_broadcast_at: HashMap::new(),
            todo_backfill_attempted: HashSet::new(),
        }
    }

    /// Removes a session from cache and emits SessionRemoved. Does not touch DB.
    fn evict(&mut self, session_id: &str, publisher: &EventPublisher) -> Option<Session> {
        let removed = self.sessions.remove(session_id)?;
        self.last_broadcast_at.remove(session_id);
        self.todo_backfill_attempted.remove(session_id);
        publisher.emit(SyncEvent::SessionRemoved {
            session_id: session_id.to_string(),
            namespace: Some(removed.namespace.clone()),
        });
        Some(removed)
    }

    /// Ensures a session is in cache, loading from DB if needed.
    fn ensure_cached(&mut self, session_id: &str, store: &Store, publisher: &EventPublisher) {
        if !self.sessions.contains_key(session_id) {
            self.refresh_session(session_id, store, publisher);
        }
    }

    pub fn get_sessions(&self) -> Vec<Session> {
        self.sessions.values().cloned().collect()
    }

    pub fn get_sessions_by_namespace(&self, namespace: &str) -> Vec<Session> {
        self.sessions
            .values()
            .filter(|s| s.namespace == namespace)
            .cloned()
            .collect()
    }

    pub fn get_session(&self, session_id: &str) -> Option<&Session> {
        self.sessions.get(session_id)
    }

    pub fn get_session_by_namespace(&self, session_id: &str, namespace: &str) -> Option<&Session> {
        self.sessions
            .get(session_id)
            .filter(|s| s.namespace == namespace)
    }

    /// Checks namespace access, falls back to DB if not cached.
    ///
    /// Returns `(session_id, Session)` or an access error.
    pub fn resolve_session_access(
        &mut self,
        session_id: &str,
        namespace: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> Result<(String, Session), SessionAccessError> {
        let session = self
            .sessions
            .get(session_id)
            .cloned()
            .or_else(|| self.refresh_session(session_id, store, publisher));

        match session {
            Some(s) if s.namespace != namespace => Err(SessionAccessError::AccessDenied),
            Some(s) => Ok((session_id.to_string(), s)),
            None => Err(SessionAccessError::NotFound),
        }
    }

    pub fn get_active_sessions(&self) -> Vec<Session> {
        self.sessions
            .values()
            .filter(|s| s.active)
            .cloned()
            .collect()
    }

    /// Finds or creates a session by tag + namespace, persists to DB and refreshes cache.
    pub fn get_or_create_session(
        &mut self,
        tag: &str,
        metadata: &Value,
        agent_state: Option<&Value>,
        namespace: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> anyhow::Result<Session> {
        use crate::store::sessions;
        let stored =
            sessions::get_or_create_session(&store.conn(), tag, metadata, agent_state, namespace)?;
        self.refresh_session(&stored.id, store, publisher)
            .ok_or_else(|| anyhow!("failed to load session"))
    }

    /// Reloads a session from DB into cache, emitting Added/Updated/Removed events.
    ///
    /// On first load, attempts to backfill todos from message history (once per session).
    /// Runtime state (active/thinking) is preserved from cache, not overwritten by DB.
    pub fn refresh_session(
        &mut self,
        session_id: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> Option<Session> {
        use crate::store::{messages, sessions};

        let mut stored = match sessions::get_session(&store.conn(), session_id) {
            Some(s) => s,
            None => {
                // DB deleted — clean up stale cache entry and notify frontend
                self.evict(session_id, publisher);
                return None;
            }
        };

        let existing = self.sessions.get(session_id);

        // To-do backfill: scan messages for TodoWrite if todos are null
        if stored.todos.is_none() && !self.todo_backfill_attempted.contains(session_id) {
            self.todo_backfill_attempted.insert(session_id.to_string());
            let flavor = stored.metadata.as_ref().and_then(|m| m.flavor);
            let msgs =
                messages::get_messages(&store.conn(), session_id, *TODO_BACKFILL_LIMIT, None);
            for msg in msgs.iter().rev() {
                if let Some(content) = &msg.content
                    && let Some(todos) = extract_todos_from_message_content(content, flavor)
                {
                    let todos_value = serde_json::to_value(&todos).ok();
                    if let Some(tv) = &todos_value
                        && sessions::set_session_todos(
                            &store.conn(),
                            session_id,
                            Some(tv),
                            msg.created_at,
                            &stored.namespace,
                        )
                    {
                        // Re-read stored session
                        if let Some(refreshed) = sessions::get_session(&store.conn(), session_id) {
                            stored = refreshed;
                        }
                    }
                    break;
                }
            }
        }

        let metadata = stored.metadata.clone();
        let agent_state = stored.agent_state.clone();
        let todos = stored.todos.clone();
        let is_update = existing.is_some();

        let session = Session {
            id: stored.id.clone(),
            namespace: stored.namespace.clone(),
            seq: stored.seq as f64,
            created_at: stored.created_at as f64,
            updated_at: stored.updated_at as f64,
            active: existing.map(|e| e.active).unwrap_or(stored.active),
            active_at: existing
                .map(|e| e.active_at)
                .unwrap_or(stored.active_at.unwrap_or(stored.created_at) as f64),
            metadata,
            metadata_version: stored.metadata_version as f64,
            agent_state,
            agent_state_version: stored.agent_state_version as f64,
            thinking: existing.map(|e| e.thinking).unwrap_or(false),
            thinking_at: existing.map(|e| e.thinking_at).unwrap_or(0.0),
            thinking_status: existing.and_then(|e| e.thinking_status.clone()),
            todos,
            permission_mode: existing.and_then(|e| e.permission_mode),
            model_mode: existing.and_then(|e| e.model_mode),
        };

        self.sessions
            .insert(session_id.to_string(), session.clone());

        if is_update {
            publisher.emit(SyncEvent::SessionUpdated {
                session_id: session_id.to_string(),
                namespace: Some(session.namespace.clone()),
                data: Some(session.clone()),
            });
        } else {
            publisher.emit(SyncEvent::SessionAdded {
                session_id: session_id.to_string(),
                namespace: Some(session.namespace.clone()),
                data: Some(session.clone()),
            });
        }

        Some(session)
    }

    /// Optimistic-concurrency metadata update; refreshes cache on success.
    pub fn update_metadata(
        &mut self,
        session_id: &str,
        metadata: &Value,
        expected_version: i64,
        namespace: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> VersionedUpdateResult<Option<Value>> {
        use crate::store::sessions;

        let result = sessions::update_session_metadata(
            &store.conn(),
            session_id,
            metadata,
            expected_version,
            namespace,
            true,
        );

        if matches!(result, VersionedUpdateResult::Success { .. }) {
            self.refresh_session(session_id, store, publisher);
        }

        result
    }

    /// Optimistic-concurrency agent_state update; refreshes cache on success.
    pub fn update_agent_state(
        &mut self,
        session_id: &str,
        agent_state: Option<&Value>,
        expected_version: i64,
        namespace: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> VersionedUpdateResult<Option<Value>> {
        use crate::store::sessions;

        let result = sessions::update_session_agent_state(
            &store.conn(),
            session_id,
            agent_state,
            expected_version,
            namespace,
        );

        if matches!(result, VersionedUpdateResult::Success { .. }) {
            self.refresh_session(session_id, store, publisher);
        }

        result
    }

    /// Full reload of all sessions from DB into cache. Called at startup.
    pub fn reload_all(&mut self, store: &Store, publisher: &EventPublisher) {
        use crate::store::sessions;
        let all = sessions::get_sessions(&store.conn());
        for s in all {
            self.refresh_session(&s.id, store, publisher);
        }
    }

    /// Processes a CLI heartbeat, updating active/thinking/mode runtime state.
    ///
    /// Only broadcasts when state actually changes or >10s since last broadcast.
    pub fn handle_session_alive(
        &mut self,
        sid: &str,
        time: i64,
        thinking: Option<bool>,
        thinking_status: Option<String>,
        permission_mode: Option<PermissionMode>,
        model_mode: Option<ModelMode>,
        store: &Store,
        publisher: &EventPublisher,
    ) {
        let t = match clamp_alive_time(time) {
            Some(t) => t,
            None => return,
        };

        self.ensure_cached(sid, store, publisher);
        let session = match self.sessions.get_mut(sid) {
            Some(s) => s,
            None => {
                tracing::warn!(session_id = %sid, "session not found after ensure_cached");
                return;
            }
        };

        let was_active = session.active;
        let was_thinking = session.thinking;
        let prev_thinking_status = session.thinking_status.clone();
        let prev_perm = session.permission_mode;
        let prev_model = session.model_mode;

        session.active = true;
        session.active_at = session.active_at.max(t as f64);
        session.thinking = thinking.unwrap_or(false);
        session.thinking_at = t as f64;
        session.thinking_status = thinking_status;
        if let Some(pm) = permission_mode {
            session.permission_mode = Some(pm);
        }
        if let Some(mm) = model_mode {
            session.model_mode = Some(mm);
        }

        let now = now_millis();
        let last = self.last_broadcast_at.get(sid).copied().unwrap_or(0);
        let mode_changed = prev_perm != session.permission_mode || prev_model != session.model_mode;
        let thinking_status_changed = prev_thinking_status != session.thinking_status;
        let should_broadcast = (!was_active && session.active)
            || (was_thinking != session.thinking)
            || thinking_status_changed
            || mode_changed
            || (now - last > 10_000);

        if should_broadcast {
            self.last_broadcast_at.insert(sid.to_string(), now);
            publisher.emit(SyncEvent::SessionUpdated {
                session_id: sid.to_string(),
                namespace: Some(session.namespace.clone()),
                data: Some(session.clone()),
            });
        }
    }

    /// Called on CLI disconnect or session end; clears active/thinking state.
    pub fn handle_session_end(
        &mut self,
        sid: &str,
        time: i64,
        store: &Store,
        publisher: &EventPublisher,
    ) {
        let t = clamp_alive_time(time).unwrap_or_else(now_millis);

        self.ensure_cached(sid, store, publisher);
        let session = match self.sessions.get_mut(sid) {
            Some(s) => s,
            None => {
                tracing::warn!(session_id = %sid, "session not found after ensure_cached");
                return;
            }
        };

        if !session.active && !session.thinking {
            return;
        }

        session.active = false;
        session.thinking = false;
        session.thinking_status = None;
        session.thinking_at = t as f64;

        publisher.emit(SyncEvent::SessionUpdated {
            session_id: sid.to_string(),
            namespace: Some(session.namespace.clone()),
            data: Some(session.clone()),
        });
    }

    /// Marks sessions inactive if no heartbeat received within `SESSION_TIMEOUT_MS`.
    pub fn expire_inactive(&mut self, publisher: &EventPublisher) {
        let now = now_millis();
        let expired: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.active && (now - s.active_at as i64) > SESSION_TIMEOUT_MS)
            .map(|(id, _)| id.clone())
            .collect();

        if !expired.is_empty() {
            info!(count = expired.len(), "expiring inactive sessions");
        }
        for id in expired {
            if let Some(session) = self.sessions.get_mut(&id) {
                debug!(session_id = %id, "session expired due to inactivity");
                session.active = false;
                session.thinking = false;
                session.thinking_status = None;
                publisher.emit(SyncEvent::SessionUpdated {
                    session_id: id,
                    namespace: Some(session.namespace.clone()),
                    data: Some(session.clone()),
                });
            }
        }
    }

    /// Applies permission_mode / model_mode from the web UI, broadcasts immediately.
    pub fn apply_session_config(
        &mut self,
        session_id: &str,
        permission_mode: Option<PermissionMode>,
        model_mode: Option<ModelMode>,
        store: &Store,
        publisher: &EventPublisher,
    ) {
        self.ensure_cached(session_id, store, publisher);
        let session = match self.sessions.get_mut(session_id) {
            Some(s) => s,
            None => {
                tracing::warn!(session_id = %session_id, "session not found after ensure_cached");
                return;
            }
        };

        if let Some(pm) = permission_mode {
            session.permission_mode = Some(pm);
        }
        if let Some(mm) = model_mode {
            session.model_mode = Some(mm);
        }

        publisher.emit(SyncEvent::SessionUpdated {
            session_id: session_id.to_string(),
            namespace: Some(session.namespace.clone()),
            data: Some(session.clone()),
        });
    }

    /// Updates metadata.name (optimistic concurrency) without touching updated_at.
    pub fn rename_session(
        &mut self,
        session_id: &str,
        name: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> anyhow::Result<()> {
        use crate::store::sessions;

        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| anyhow!("session not found"))?;

        let mut new_meta = session
            .metadata
            .clone()
            .ok_or_else(|| anyhow!("session has no metadata"))?;
        new_meta.name = Some(name.to_string());
        let new_meta_value = serde_json::to_value(&new_meta)?;

        let version = session.metadata_version as i64;
        let namespace = session.namespace.clone();

        let result = sessions::update_session_metadata(
            &store.conn(),
            session_id,
            &new_meta_value,
            version,
            &namespace,
            false,
        );

        use crate::store::types::VersionedUpdateResult;
        match result {
            VersionedUpdateResult::Success { .. } => {
                self.refresh_session(session_id, store, publisher);
                Ok(())
            }
            VersionedUpdateResult::VersionMismatch { .. } => {
                Err(anyhow!("session was modified concurrently"))
            }
            VersionedUpdateResult::Error => Err(anyhow!("failed to update session metadata")),
        }
    }

    /// Deletes an inactive session, cleaning up cache and emitting SessionRemoved.
    ///
    /// Returns an error if the session is still active (unless `force` is true).
    pub fn delete_session(
        &mut self,
        session_id: &str,
        store: &Store,
        publisher: &EventPublisher,
        force: bool,
    ) -> anyhow::Result<()> {
        use crate::store::sessions;

        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| anyhow!("session not found"))?;

        if !force && session.active {
            return Err(anyhow!("cannot delete active session"));
        }

        let namespace = session.namespace.clone();

        if !sessions::delete_session(&store.conn(), session_id, &namespace) {
            return Err(anyhow!("failed to delete session"));
        }

        self.evict(session_id, publisher);

        Ok(())
    }

    /// Merges messages, metadata, and todos from an old session into a new one, then deletes the old.
    ///
    /// Used when a CLI reconnect creates a new session due to tag change.
    /// Metadata merge preserves name/summary/worktree/path/host from old if missing in new.
    pub fn merge_sessions(
        &mut self,
        old_session_id: &str,
        new_session_id: &str,
        namespace: &str,
        store: &Store,
        publisher: &EventPublisher,
    ) -> anyhow::Result<()> {
        use crate::store::{messages, sessions};

        if old_session_id == new_session_id {
            return Ok(());
        }

        let old_stored =
            sessions::get_session_by_namespace(&store.conn(), old_session_id, namespace)
                .ok_or_else(|| anyhow!("old session not found"))?;
        let new_stored =
            sessions::get_session_by_namespace(&store.conn(), new_session_id, namespace)
                .ok_or_else(|| anyhow!("new session not found"))?;

        // Merge messages
        let (moved, old_max_seq, new_max_seq) =
            messages::merge_session_messages(&store.conn(), old_session_id, new_session_id)?;
        info!(
            old_session_id = old_session_id,
            new_session_id = new_session_id,
            moved,
            old_max_seq,
            new_max_seq,
            "[mergeSessions] messages merged"
        );

        // Merge metadata
        if let (Some(old_meta), Some(new_meta)) = (&old_stored.metadata, &new_stored.metadata)
            && let (Ok(old_val), Ok(new_val)) = (
                serde_json::to_value(old_meta),
                serde_json::to_value(new_meta),
            )
            && let Some(merged) = merge_metadata(&old_val, &new_val)
        {
            // Try up to 2 times for optimistic concurrency
            for _ in 0..2 {
                if let Some(latest) =
                    sessions::get_session_by_namespace(&store.conn(), new_session_id, namespace)
                {
                    let result = sessions::update_session_metadata(
                        &store.conn(),
                        new_session_id,
                        &merged,
                        latest.metadata_version,
                        namespace,
                        false,
                    );
                    use crate::store::types::VersionedUpdateResult;
                    match result {
                        VersionedUpdateResult::Success { .. } => break,
                        VersionedUpdateResult::Error => break,
                        _ => continue,
                    }
                }
            }
        }

        // Merge todos
        if old_stored.todos.is_some()
            && let Some(ts) = old_stored.todos_updated_at
        {
            let todos_value = serde_json::to_value(&old_stored.todos).ok();
            sessions::set_session_todos(
                &store.conn(),
                new_session_id,
                todos_value.as_ref(),
                ts,
                namespace,
            );
        }

        // Delete old session
        info!(
            old_session_id = old_session_id,
            new_session_id = new_session_id,
            "[mergeSessions] deleting old session"
        );
        self.delete_session(old_session_id, store, publisher, true)?;

        self.refresh_session(new_session_id, store, publisher);
        Ok(())
    }
}

/// Merge old metadata into new, preserving name/summary/worktree/path/host from old if missing in new.
fn merge_metadata(old: &Value, new: &Value) -> Option<Value> {
    let old_obj = old.as_object()?;
    let new_obj = new.as_object()?;
    let mut merged = new_obj.clone();
    let mut changed = false;

    // Preserve name
    if old_obj.get("name").and_then(|v| v.as_str()).is_some()
        && new_obj.get("name").and_then(|v| v.as_str()).is_none()
    {
        merged.insert("name".into(), old_obj["name"].clone());
        changed = true;
    }

    // Preserve summary if newer
    let old_updated = old_obj
        .get("summary")
        .and_then(|s| s.get("updatedAt"))
        .and_then(|v| v.as_f64());
    let new_updated = new_obj
        .get("summary")
        .and_then(|s| s.get("updatedAt"))
        .and_then(|v| v.as_f64());
    if let Some(old_t) = old_updated
        && (new_updated.is_none() || old_t > new_updated.unwrap_or(0.0))
        && let Some(summary) = old_obj.get("summary")
    {
        merged.insert("summary".into(), summary.clone());
        changed = true;
    }

    // Preserve worktree
    if old_obj.contains_key("worktree") && !new_obj.contains_key("worktree") {
        merged.insert("worktree".into(), old_obj["worktree"].clone());
        changed = true;
    }

    // Preserve path/host
    for key in &["path", "host"] {
        if old_obj.get(*key).and_then(|v| v.as_str()).is_some()
            && new_obj.get(*key).and_then(|v| v.as_str()).is_none()
        {
            merged.insert((*key).to_string(), old_obj[*key].clone());
            changed = true;
        }
    }

    if changed {
        Some(Value::Object(merged))
    } else {
        None
    }
}
