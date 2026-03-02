use hapir_shared::common::sync_event::SyncEvent;
use tokio::sync::{broadcast, mpsc};

use super::sse_manager::{SseManager, SseMessage, SseSubscription};
use super::visibility_tracker::VisibilityState;

pub type SyncEventListener = Box<dyn Fn(&SyncEvent) + Send + Sync>;

/// Publishes sync events to listeners and SSE connections.
/// Uses a tokio broadcast channel for fan-out.
/// SseManager handles its own concurrency internally (DashMap + VisibilityTracker RwLock),
/// so no outer lock is needed.
pub struct EventPublisher {
    tx: broadcast::Sender<SyncEvent>,
    sse_manager: SseManager,
}

impl EventPublisher {
    pub fn new(sse_manager: SseManager) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx, sse_manager }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.tx.subscribe()
    }

    pub fn subscribe_sse(
        &self,
        id: String,
        namespace: String,
        all: bool,
        session_id: Option<String>,
        machine_id: Option<String>,
        visibility: VisibilityState,
    ) -> (mpsc::UnboundedReceiver<SseMessage>, SseSubscription) {
        self.sse_manager
            .subscribe(id, namespace, all, session_id, machine_id, visibility)
    }

    pub fn unsubscribe_sse(&self, id: &str) {
        self.sse_manager.unsubscribe(id);
    }

    pub fn sse_connection_count(&self) -> usize {
        self.sse_manager.connection_count()
    }

    pub fn has_visible_sse_connection(&self, namespace: &str) -> bool {
        self.sse_manager
            .visibility()
            .has_visible_connection(namespace)
    }

    pub fn set_sse_visibility(
        &self,
        subscription_id: &str,
        namespace: &str,
        state: VisibilityState,
    ) -> bool {
        self.sse_manager
            .visibility()
            .set_visibility(subscription_id, namespace, state)
    }

    pub fn send_heartbeats(&self) {
        self.sse_manager.send_heartbeats();
    }

    pub fn send_toast(&self, namespace: &str, event: &SyncEvent) -> usize {
        self.sse_manager.send_toast(namespace, event)
    }

    pub fn heartbeat_ms(&self) -> u64 {
        self.sse_manager.heartbeat_ms()
    }

    pub fn emit(&self, event: SyncEvent) {
        self.sse_manager.broadcast(&event);
        let _ = self.tx.send(event);
    }
}
