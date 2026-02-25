use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use hapir_shared::schemas::SyncEvent;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, trace, warn};

use crate::sync::SyncEngine;

use super::message_event_type::extract_message_event_type;
use super::push_channel::NotificationChannel;

struct HubState {
    last_known_requests: HashMap<String, HashSet<String>>,
    notification_debounce: HashMap<String, JoinHandle<()>>,
    last_ready_notification: HashMap<String, i64>,
}

pub struct NotificationHub {
    channels: Vec<Arc<dyn NotificationChannel>>,
    state: Mutex<HubState>,
    ready_cooldown_ms: i64,
    permission_debounce_ms: u64,
}

impl NotificationHub {
    pub fn new(
        channels: Vec<Arc<dyn NotificationChannel>>,
        ready_cooldown_ms: i64,
        permission_debounce_ms: u64,
    ) -> Self {
        Self {
            channels,
            state: Mutex::new(HubState {
                last_known_requests: HashMap::new(),
                notification_debounce: HashMap::new(),
                last_ready_notification: HashMap::new(),
            }),
            ready_cooldown_ms,
            permission_debounce_ms,
        }
    }

    /// Start processing sync events in a background task.
    ///
    /// Subscribes to `SyncEngine`'s broadcast channel and dispatches
    /// notifications based on incoming events.
    pub async fn start(self: Arc<Self>, sync_engine: Arc<SyncEngine>) {
        let mut rx = sync_engine.subscribe();

        let hub = self.clone();
        let engine = sync_engine.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        hub.handle_sync_event(&event, &engine).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        warn!(count, "notification hub lagged behind broadcast channel");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        trace!("notification hub broadcast channel closed; stopping");
                        break;
                    }
                }
            }
        });
    }

    async fn handle_sync_event(&self, event: &SyncEvent, sync_engine: &Arc<SyncEngine>) {
        match event {
            SyncEvent::SessionUpdated { session_id, .. }
            | SyncEvent::SessionAdded { session_id, .. } => {
                let session = sync_engine.get_session(session_id).await;
                let Some(session) = session else {
                    self.clear_session_state(session_id).await;
                    return;
                };
                if !session.active {
                    self.clear_session_state(session_id).await;
                    return;
                }
                self.check_for_permission_notification(&session, sync_engine)
                    .await;
            }

            SyncEvent::SessionRemoved { session_id, .. } => {
                self.clear_session_state(session_id).await;
            }

            SyncEvent::MessageReceived {
                session_id,
                message,
                ..
            } => {
                let event_type = extract_message_event_type(&message.content);
                if event_type.as_deref() == Some("ready") {
                    self.send_ready_notification(session_id, sync_engine).await;
                }
            }

            _ => {}
        }
    }

    async fn clear_session_state(&self, session_id: &str) {
        let mut state = self.state.lock().await;
        if let Some(handle) = state.notification_debounce.remove(session_id) {
            handle.abort();
        }
        state.last_known_requests.remove(session_id);
        state.last_ready_notification.remove(session_id);
    }

    async fn check_for_permission_notification(
        &self,
        session: &hapir_shared::schemas::Session,
        sync_engine: &Arc<SyncEngine>,
    ) {
        let requests = match session
            .agent_state
            .as_ref()
            .and_then(|s| s.requests.as_ref())
        {
            Some(r) => r,
            None => return,
        };

        let new_request_ids: HashSet<String> = requests.keys().cloned().collect();

        let has_new_requests = {
            let mut state = self.state.lock().await;
            let old_request_ids = state
                .last_known_requests
                .get(&session.id)
                .cloned()
                .unwrap_or_default();

            let has_new = new_request_ids
                .iter()
                .any(|id| !old_request_ids.contains(id));

            state
                .last_known_requests
                .insert(session.id.clone(), new_request_ids);

            has_new
        };

        if !has_new_requests {
            return;
        }

        let session_id = session.id.clone();
        let session_id_for_map = session_id.clone();
        let debounce_ms = self.permission_debounce_ms;
        let channels = self.channels.clone();
        let engine = sync_engine.clone();

        {
            let mut state = self.state.lock().await;
            if let Some(handle) = state.notification_debounce.remove(&session_id) {
                handle.abort();
            }
        }

        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(debounce_ms)).await;

            let session = engine.get_session(&session_id).await;

            let Some(session) = session else {
                return;
            };
            if !session.active {
                return;
            }

            for channel in &channels {
                if let Err(e) = channel.send_permission_request(&session).await {
                    error!(
                        error = %e,
                        session_id = %session.id,
                        "failed to send permission notification"
                    );
                }
            }
        });

        {
            let mut state = self.state.lock().await;
            state
                .notification_debounce
                .insert(session_id_for_map, handle);
        }
    }

    async fn send_ready_notification(&self, session_id: &str, sync_engine: &Arc<SyncEngine>) {
        let session = sync_engine.get_session(session_id).await;

        let Some(session) = session else {
            return;
        };
        if !session.active {
            return;
        }

        // Throttle: check cooldown
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        {
            let mut state = self.state.lock().await;
            let last = state
                .last_ready_notification
                .get(session_id)
                .copied()
                .unwrap_or(0);
            if now - last < self.ready_cooldown_ms {
                return;
            }
            state
                .last_ready_notification
                .insert(session_id.to_string(), now);
        }

        // Send ready notification to all channels
        for channel in &self.channels {
            if let Err(e) = channel.send_ready(&session).await {
                error!(
                    error = %e,
                    session_id = %session.id,
                    "failed to send ready notification"
                );
            }
        }
    }
}

impl Drop for NotificationHub {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        for (_, handle) in state.notification_debounce.drain() {
            handle.abort();
        }
    }
}
