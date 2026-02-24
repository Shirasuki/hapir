use super::super::connection_manager::ConnectionManager;
use super::super::terminal_registry::TerminalRegistry;
use crate::sync::{SyncEngine, VersionedUpdateResult};
use hapir_shared::schemas::{MessageDeltaData, SyncEvent};
use hapir_shared::socket::{
    MachineAliveRequest, MachineUpdateMetadataRequest, MachineUpdateStateRequest,
    RpcRegisterRequest, SessionAliveRequest, SessionEndRequest,
};
use hapir_shared::ws_protocol::{WsBroadcast, WsMessage};
use serde_json::Value;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use tracing::debug;

async fn emit_access_error(
    conn_mgr: &ConnectionManager,
    conn_id: &str,
    scope: &str,
    id: &str,
    reason: &str,
) {
    let msg = WsMessage::event(
        "error",
        serde_json::json!({
            "message": format!("{scope} {reason}"), "code": reason, "scope": scope, "id": id
        }),
    );
    conn_mgr
        .send_to(conn_id, &serde_json::to_string(&msg).unwrap_or_default())
        .await;
}

/// Process an incoming WebSocket event from a CLI connection.
pub async fn handle_cli_event(
    conn_id: &str,
    namespace: &str,
    event: &str,
    data: Value,
    _request_id: Option<&str>,
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
    terminal_registry: &Arc<RwLock<TerminalRegistry>>,
) -> Option<Value> {
    match event {
        "message" => handle_message(conn_id, namespace, data, sync_engine, conn_mgr).await,
        "message-delta" => {
            handle_message_delta(conn_id, namespace, data, sync_engine, conn_mgr).await;
            None
        }
        "update-metadata" => {
            handle_update_metadata(namespace, data, sync_engine, conn_id, conn_mgr).await
        }
        "update-state" => {
            handle_update_state(namespace, data, sync_engine, conn_id, conn_mgr).await
        }
        "session-alive" => {
            handle_session_alive(conn_id, namespace, data, sync_engine, conn_mgr).await;
            None
        }
        "session-end" => {
            handle_session_end(conn_id, namespace, data, sync_engine, conn_mgr).await;
            None
        }
        "machine-alive" => {
            handle_machine_alive(conn_id, namespace, data, sync_engine, conn_mgr).await;
            None
        }
        "machine-update-metadata" => {
            handle_machine_update_metadata(namespace, data, sync_engine, conn_id, conn_mgr).await
        }
        "machine-update-state" => {
            handle_machine_update_state(namespace, data, sync_engine, conn_id, conn_mgr).await
        }
        "rpc-register" => {
            if let Ok(req) = serde_json::from_value::<RpcRegisterRequest>(data) {
                conn_mgr.rpc_register(conn_id, &req.method).await;
            }
            None
        }
        "rpc-unregister" => {
            if let Ok(req) = serde_json::from_value::<RpcRegisterRequest>(data) {
                conn_mgr.rpc_unregister(conn_id, &req.method).await;
            }
            None
        }
        "rpc-response" | "rpc-request:ack" => None,
        "terminal:ready" | "terminal:output" => {
            forward_terminal_event(conn_id, event, &data, conn_mgr, terminal_registry, true).await;
            None
        }
        "terminal:error" => {
            handle_terminal_error_from_cli(conn_id, &data, conn_mgr, terminal_registry).await;
            None
        }
        "terminal:exit" => {
            handle_terminal_exit_from_cli(conn_id, &data, conn_mgr, terminal_registry).await;
            None
        }
        "ping" => Some(Value::Null),
        _ => {
            debug!(event, "unhandled CLI event");
            None
        }
    }
}

async fn handle_message(
    conn_id: &str,
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let sid = data.get("sid").and_then(|v| v.as_str())?;
    let local_id = data.get("localId").and_then(|v| v.as_str());

    if let Err(reason) = sync_engine.resolve_session_access(sid, namespace).await {
        emit_access_error(conn_mgr, conn_id, "session", sid, reason).await;
        return None;
    }

    let raw = data.get("message")?;
    let content = if let Some(s) = raw.as_str() {
        serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
    } else {
        raw.clone()
    };

    let msg = match sync_engine.add_cli_message(sid, namespace, &content, local_id) {
        Ok(m) => m,
        Err(_) => return None,
    };

    let broadcast = WsBroadcast::new(
        msg.seq,
        msg.created_at,
        serde_json::json!({
            "t": "new-message",
            "sid": sid,
            "message": {
                "id": msg.id,
                "seq": msg.seq,
                "createdAt": msg.created_at,
                "localId": msg.local_id,
                "content": msg.content,
            }
        }),
    );
    conn_mgr
        .broadcast_to_session(sid, &broadcast.to_ws_string(), Some(conn_id))
        .await;

    None
}

async fn handle_message_delta(
    conn_id: &str,
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
) {
    let sid = match data.get("sid").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return,
    };

    if let Err(reason) = sync_engine.resolve_session_access(sid, namespace).await {
        emit_access_error(conn_mgr, conn_id, "session", sid, reason).await;
        return;
    }

    let delta = match data.get("delta") {
        Some(d) => d.clone(),
        None => return,
    };

    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let broadcast = WsBroadcast::new(
        now,
        now,
        serde_json::json!({
            "t": "message-delta",
            "sid": sid,
            "delta": delta,
        }),
    );
    conn_mgr
        .broadcast_to_session(sid, &broadcast.to_ws_string(), Some(conn_id))
        .await;

    if let Ok(delta_data) = serde_json::from_value::<MessageDeltaData>(delta) {
        sync_engine
            .handle_realtime_event(SyncEvent::MessageDelta {
                session_id: sid.to_string(),
                namespace: Some(namespace.to_string()),
                delta: delta_data,
            })
            .await;
    }
}

async fn handle_update_metadata(
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_id: &str,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let sid = data.get("sid").and_then(|v| v.as_str())?;
    let expected_version = data.get("expectedVersion").and_then(|v| v.as_i64())?;
    let metadata = data.get("metadata")?;

    if let Err(reason) = sync_engine.resolve_session_access(sid, namespace).await {
        return Some(serde_json::json!({"result": "error", "reason": reason}));
    }

    let result = sync_engine
        .update_session_metadata(sid, metadata, expected_version, namespace)
        .await;

    let response = match &result {
        VersionedUpdateResult::Success { version, value } => {
            let now = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            let broadcast = WsBroadcast::new(
                now,
                now,
                serde_json::json!({
                    "t": "update-session",
                    "sid": sid,
                    "metadata": {"version": version, "value": metadata},
                    "agentState": null,
                }),
            );
            conn_mgr
                .broadcast_to_session(sid, &broadcast.to_ws_string(), Some(conn_id))
                .await;
            serde_json::json!({"result": "success", "version": version, "metadata": value})
        }
        VersionedUpdateResult::VersionMismatch { version, value } => {
            serde_json::json!({"result": "version-mismatch", "version": version, "metadata": value})
        }
        VersionedUpdateResult::Error => {
            serde_json::json!({"result": "error"})
        }
    };

    Some(response)
}

async fn handle_update_state(
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_id: &str,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let sid = data.get("sid").and_then(|v| v.as_str())?;
    let expected_version = data.get("expectedVersion").and_then(|v| v.as_i64())?;
    let agent_state = data.get("agentState");

    if let Err(reason) = sync_engine.resolve_session_access(sid, namespace).await {
        return Some(serde_json::json!({"result": "error", "reason": reason}));
    }

    let result = sync_engine
        .update_session_agent_state(sid, agent_state, expected_version, namespace)
        .await;

    let response = match &result {
        VersionedUpdateResult::Success { version, value } => {
            let now = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            let broadcast = WsBroadcast::new(
                now,
                now,
                serde_json::json!({
                    "t": "update-session",
                    "sid": sid,
                    "metadata": null,
                    "agentState": {"version": version, "value": agent_state},
                }),
            );
            conn_mgr
                .broadcast_to_session(sid, &broadcast.to_ws_string(), Some(conn_id))
                .await;
            serde_json::json!({"result": "success", "version": version, "agentState": value})
        }
        VersionedUpdateResult::VersionMismatch { version, value } => {
            serde_json::json!({"result": "version-mismatch", "version": version, "agentState": value})
        }
        VersionedUpdateResult::Error => {
            serde_json::json!({"result": "error"})
        }
    };

    Some(response)
}

async fn handle_session_alive(
    conn_id: &str,
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
) {
    let req: SessionAliveRequest = match serde_json::from_value(data) {
        Ok(r) => r,
        Err(_) => return,
    };

    if let Err(reason) = sync_engine.resolve_session_access(&req.sid, namespace).await {
        emit_access_error(conn_mgr, conn_id, "session", &req.sid, reason).await;
        return;
    }

    sync_engine
        .handle_session_alive(
            &req.sid,
            req.time,
            Some(req.thinking),
            req.thinking_status,
            req.permission_mode,
            req.model_mode,
        )
        .await;
}

async fn handle_session_end(
    conn_id: &str,
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
) {
    let req: SessionEndRequest = match serde_json::from_value(data) {
        Ok(r) => r,
        Err(_) => return,
    };

    if let Err(reason) = sync_engine
        .resolve_session_access(&req.sid, namespace)
        .await
    {
        emit_access_error(conn_mgr, conn_id, "session", &req.sid, reason).await;
        return;
    }

    sync_engine.handle_session_end(&req.sid, req.time).await;
}

async fn handle_machine_alive(
    conn_id: &str,
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
) {
    let req: MachineAliveRequest = match serde_json::from_value(data) {
        Ok(r) => r,
        Err(_) => return,
    };

    if sync_engine
        .get_machine_by_namespace(&req.machine_id, namespace)
        .await
        .is_none()
    {
        emit_access_error(conn_mgr, conn_id, "machine", &req.machine_id, "not-found").await;
        return;
    }

    sync_engine
        .handle_machine_alive(&req.machine_id, req.time)
        .await;
}

async fn handle_machine_update_metadata(
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_id: &str,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let req: MachineUpdateMetadataRequest = serde_json::from_value(data).ok()?;
    let mid = &req.machine_id;
    let metadata = serde_json::to_value(&req.metadata).ok()?;

    if sync_engine
        .get_machine_by_namespace(mid, namespace)
        .await
        .is_none()
    {
        return Some(serde_json::json!({"result": "error", "reason": "not-found"}));
    }

    let result = sync_engine
        .update_machine_metadata(mid, &metadata, req.expected_version, namespace)
        .await;

    let response = match &result {
        VersionedUpdateResult::Success { version, value } => {
            let now = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            let broadcast = WsBroadcast::new(
                now,
                now,
                serde_json::json!({
                    "t": "update-machine",
                    "machineId": mid,
                    "metadata": {"version": version, "value": metadata},
                    "runnerState": null,
                }),
            );
            conn_mgr
                .broadcast_to_machine(mid, &broadcast.to_ws_string(), Some(conn_id))
                .await;
            serde_json::json!({"result": "success", "version": version, "metadata": value})
        }
        VersionedUpdateResult::VersionMismatch { version, value } => {
            serde_json::json!({"result": "version-mismatch", "version": version, "metadata": value})
        }
        VersionedUpdateResult::Error => {
            serde_json::json!({"result": "error"})
        }
    };

    Some(response)
}

async fn handle_machine_update_state(
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_id: &str,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let req: MachineUpdateStateRequest = serde_json::from_value(data).ok()?;
    let mid = &req.machine_id;
    let runner_state = &req.runner_state;

    if sync_engine
        .get_machine_by_namespace(mid, namespace)
        .await
        .is_none()
    {
        return Some(serde_json::json!({"result": "error", "reason": "not-found"}));
    }

    let result = sync_engine
        .update_machine_runner_state(mid, Some(runner_state), req.expected_version, namespace)
        .await;

    let response = match &result {
        VersionedUpdateResult::Success { version, value } => {
            let now = SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            let broadcast = WsBroadcast::new(
                now,
                now,
                serde_json::json!({
                    "t": "update-machine",
                    "machineId": mid,
                    "metadata": null,
                    "runnerState": {"version": version, "value": runner_state},
                }),
            );
            conn_mgr
                .broadcast_to_machine(mid, &broadcast.to_ws_string(), Some(conn_id))
                .await;
            serde_json::json!({"result": "success", "version": version, "runnerState": value})
        }
        VersionedUpdateResult::VersionMismatch { version, value } => {
            serde_json::json!({"result": "version-mismatch", "version": version, "runnerState": value})
        }
        VersionedUpdateResult::Error => {
            serde_json::json!({"result": "error"})
        }
    };

    Some(response)
}

// ---------- Terminal event forwarding (CLI → webapp) ----------

#[inline]
async fn forward_terminal_event(
    conn_id: &str,
    event: &str,
    data: &Value,
    conn_mgr: &Arc<ConnectionManager>,
    terminal_registry: &Arc<RwLock<TerminalRegistry>>,
    should_mark_activity: bool,
) {
    let terminal_id = match data.get("terminalId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return,
    };

    let entry = {
        let reg = terminal_registry.read().await;
        match reg.get(&terminal_id) {
            Some(e) if e.cli_socket_id == conn_id => e.clone(),
            _ => return,
        }
    };

    if data.get("sessionId").and_then(|v| v.as_str()) != Some(&entry.session_id) {
        return;
    }

    if should_mark_activity {
        terminal_registry.write().await.mark_activity(&terminal_id);
    }

    let msg = WsMessage::event(event, data.clone());
    let msg_str = serde_json::to_string(&msg).unwrap_or_default();
    conn_mgr.send_to(&entry.socket_id, &msg_str).await;
}

async fn handle_terminal_exit_from_cli(
    conn_id: &str,
    data: &Value,
    conn_mgr: &Arc<ConnectionManager>,
    terminal_registry: &Arc<RwLock<TerminalRegistry>>,
) {
    let terminal_id = match data.get("terminalId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return,
    };

    let entry = {
        let reg = terminal_registry.read().await;
        match reg.get(&terminal_id) {
            Some(e) if e.cli_socket_id == conn_id => e.clone(),
            _ => return,
        }
    };

    if data.get("sessionId").and_then(|v| v.as_str()) != Some(&entry.session_id) {
        return;
    }

    terminal_registry.write().await.remove(&terminal_id);

    let msg = WsMessage::event("terminal:exit", data.clone());
    let msg_str = serde_json::to_string(&msg).unwrap_or_default();
    conn_mgr.send_to(&entry.socket_id, &msg_str).await;
}

async fn handle_terminal_error_from_cli(
    conn_id: &str,
    data: &Value,
    conn_mgr: &Arc<ConnectionManager>,
    terminal_registry: &Arc<RwLock<TerminalRegistry>>,
) {
    let terminal_id = match data.get("terminalId").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return,
    };

    let entry = {
        let reg = terminal_registry.read().await;
        match reg.get(&terminal_id) {
            Some(e) if e.cli_socket_id == conn_id => e.clone(),
            _ => return,
        }
    };

    if data.get("sessionId").and_then(|v| v.as_str()) != Some(&entry.session_id) {
        return;
    }

    terminal_registry.write().await.remove(&terminal_id);

    let msg = WsMessage::event("terminal:error", data.clone());
    let msg_str = serde_json::to_string(&msg).unwrap_or_default();
    conn_mgr.send_to(&entry.socket_id, &msg_str).await;
}
