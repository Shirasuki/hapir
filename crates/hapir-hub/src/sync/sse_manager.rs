use dashmap::DashMap;
use hapir_shared::schemas::SyncEvent;
use tokio::sync::mpsc;
use tracing::debug;

use super::visibility_tracker::{VisibilityState, VisibilityTracker};

/// An SSE subscription descriptor.
#[derive(Debug, Clone)]
pub struct SseSubscription {
    pub id: String,
    pub namespace: String,
    pub all: bool,
    pub session_id: Option<String>,
    pub machine_id: Option<String>,
}

/// Internal connection state.
#[derive(Clone)]
struct SseConnection {
    sub: SseSubscription,
    tx: mpsc::UnboundedSender<SseMessage>,
}

/// Messages sent to an SSE connection.
#[derive(Debug, Clone)]
pub enum SseMessage {
    Event(SyncEvent),
    Heartbeat,
}

/// Manages Server-Sent Events connections.
/// All methods take `&self`; concurrency is handled by DashMap shards
/// and VisibilityTracker's internal RwLock.
pub struct SseManager {
    connections: DashMap<String, SseConnection>,
    visibility: VisibilityTracker,
    heartbeat_ms: u64,
}

impl SseManager {
    pub fn new(heartbeat_ms: u64) -> Self {
        Self {
            connections: DashMap::new(),
            visibility: VisibilityTracker::new(),
            heartbeat_ms,
        }
    }

    pub fn heartbeat_ms(&self) -> u64 {
        self.heartbeat_ms
    }

    pub fn visibility(&self) -> &VisibilityTracker {
        &self.visibility
    }

    /// Subscribe a new SSE connection. Returns a receiver for events and the subscription info.
    pub fn subscribe(
        &self,
        id: String,
        namespace: String,
        all: bool,
        session_id: Option<String>,
        machine_id: Option<String>,
        visibility: VisibilityState,
    ) -> (mpsc::UnboundedReceiver<SseMessage>, SseSubscription) {
        let (tx, rx) = mpsc::unbounded_channel();

        let sub = SseSubscription {
            id: id.clone(),
            namespace: namespace.clone(),
            all,
            session_id,
            machine_id,
        };

        self.connections.insert(
            id.clone(),
            SseConnection {
                sub: sub.clone(),
                tx,
            },
        );
        self.visibility
            .register_connection(&id, &namespace, visibility);

        debug!(
            subscription_id = %id,
            %namespace,
            all,
            session_id = ?sub.session_id,
            machine_id = ?sub.machine_id,
            total = self.connections.len(),
            "SSE subscribe"
        );

        (rx, sub)
    }

    pub fn unsubscribe(&self, id: &str) {
        let had = self.connections.remove(id).is_some();
        self.visibility.remove_connection(id);
        debug!(
            subscription_id = %id,
            removed = had,
            remaining = self.connections.len(),
            "SSE unsubscribe"
        );
    }

    /// Take a snapshot of all connections so we don't hold shard locks during sends.
    /// This avoids issues where concurrent subscribe/unsubscribe operations could
    /// cause the DashMap iterator to skip entries in modified shards.
    fn snapshot(&self) -> Vec<SseConnection> {
        self.connections
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Broadcast an event to all matching connections.
    /// Returns IDs of connections that failed delivery (for deferred cleanup).
    pub fn broadcast(&self, event: &SyncEvent) -> Vec<String> {
        let conns = self.snapshot();
        let event_type = event_type_name(event);
        let event_sid = event_session_id(event);
        let mut matched = 0usize;
        let mut failed = Vec::new();
        for conn in &conns {
            if !should_send(&conn.sub, event) {
                continue;
            }
            matched += 1;
            if conn.tx.send(SseMessage::Event(event.clone())).is_err() {
                failed.push(conn.sub.id.clone());
            }
        }
        debug!(
            event_type,
            event_session_id = ?event_sid,
            total_conns = conns.len(),
            matched,
            failed = failed.len(),
            "SSE broadcast"
        );
        failed
    }

    /// Send a toast to visible connections in a namespace. Returns delivery count.
    pub fn send_toast(&self, namespace: &str, event: &SyncEvent) -> usize {
        let conns = self.snapshot();
        let mut count = 0;
        let mut failed = Vec::new();
        for conn in &conns {
            if conn.sub.namespace != namespace {
                continue;
            }
            if !self.visibility.is_visible_connection(&conn.sub.id) {
                continue;
            }
            if conn.tx.send(SseMessage::Event(event.clone())).is_ok() {
                count += 1;
            } else {
                failed.push(conn.sub.id.clone());
            }
        }
        for id in failed {
            self.unsubscribe(&id);
        }
        count
    }

    /// Send heartbeat to all connections. Auto-unsubscribes failed connections.
    pub fn send_heartbeats(&self) {
        let conns = self.snapshot();
        let mut failed = Vec::new();
        for conn in &conns {
            if conn.tx.send(SseMessage::Heartbeat).is_err() {
                failed.push(conn.sub.id.clone());
            }
        }
        for id in failed {
            self.unsubscribe(&id);
        }
    }

    /// Clean up connections whose senders have been dropped.
    pub fn cleanup_dead(&self) {
        let conns = self.snapshot();
        for conn in &conns {
            if conn.tx.is_closed() {
                self.unsubscribe(&conn.sub.id);
            }
        }
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

fn should_send(sub: &SseSubscription, event: &SyncEvent) -> bool {
    // connection-changed goes to everyone
    if matches!(event, SyncEvent::ConnectionChanged { .. }) {
        return true;
    }

    // Check namespace match
    let event_ns = event_namespace(event);
    if let Some(ns) = event_ns {
        if ns != sub.namespace {
            return false;
        }
    } else {
        return false;
    }

    // "all" subscribers get everything in their namespace
    if sub.all {
        return true;
    }

    // message-received only goes to the specific session subscriber
    if let SyncEvent::MessageReceived { session_id, .. } = event {
        return sub.session_id.as_deref() == Some(session_id.as_str());
    }

    // Check session/machine match
    if let Some(sid) = event_session_id(event)
        && sub.session_id.as_deref() == Some(sid)
    {
        return true;
    }
    if let Some(mid) = event_machine_id(event)
        && sub.machine_id.as_deref() == Some(mid)
    {
        return true;
    }

    false
}

fn event_namespace(event: &SyncEvent) -> Option<&str> {
    match event {
        SyncEvent::SessionAdded { namespace, .. }
        | SyncEvent::SessionUpdated { namespace, .. }
        | SyncEvent::SessionRemoved { namespace, .. }
        | SyncEvent::MessageReceived { namespace, .. }
        | SyncEvent::MessageDelta { namespace, .. }
        | SyncEvent::MachineUpdated { namespace, .. }
        | SyncEvent::Toast { namespace, .. }
        | SyncEvent::ConnectionChanged { namespace, .. } => namespace.as_deref(),
    }
}

fn event_session_id(event: &SyncEvent) -> Option<&str> {
    match event {
        SyncEvent::SessionAdded { session_id, .. }
        | SyncEvent::SessionUpdated { session_id, .. }
        | SyncEvent::SessionRemoved { session_id, .. }
        | SyncEvent::MessageReceived { session_id, .. }
        | SyncEvent::MessageDelta { session_id, .. } => Some(session_id.as_str()),
        _ => None,
    }
}

fn event_machine_id(event: &SyncEvent) -> Option<&str> {
    match event {
        SyncEvent::MachineUpdated { machine_id, .. } => Some(machine_id.as_str()),
        _ => None,
    }
}

fn event_type_name(event: &SyncEvent) -> &'static str {
    match event {
        SyncEvent::SessionAdded { .. } => "session-added",
        SyncEvent::SessionUpdated { .. } => "session-updated",
        SyncEvent::SessionRemoved { .. } => "session-removed",
        SyncEvent::MessageReceived { .. } => "message-received",
        SyncEvent::MessageDelta { .. } => "message-delta",
        SyncEvent::MachineUpdated { .. } => "machine-updated",
        SyncEvent::Toast { .. } => "toast",
        SyncEvent::ConnectionChanged { .. } => "connection-changed",
    }
}
