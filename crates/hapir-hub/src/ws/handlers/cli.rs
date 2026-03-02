use super::super::connection_manager::ConnectionManager;
use super::super::terminal_registry::TerminalRegistry;
use crate::sync::{SyncEngine, VersionedUpdateResult};
use hapir_shared::cli::socket::{
    MachineAliveRequest, MachineUpdateMetadataRequest, MachineUpdateStateRequest,
    RpcRegisterRequest, SessionAliveRequest, SessionEndRequest, SessionMessageDeltaRequest,
    SessionMessageRequest, SessionUpdateMetadataRequest, SessionUpdateStateRequest, SocketError,
    SocketErrorReason, SocketErrorScope, UpdateBody, UpdateMessage, VersionedUpdateResponse,
    VersionedValue,
};
use hapir_shared::cli::ws_protocol::{WsBroadcast, WsMessage};
use hapir_shared::common::sync_event::{MessageDeltaData, SyncEvent};
use hapir_shared::common::utils::now_millis;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

async fn emit_access_error(
    conn_mgr: &ConnectionManager,
    conn_id: &str,
    scope: SocketErrorScope,
    id: &str,
    reason: SocketErrorReason,
) {
    let err = SocketError {
        message: format!("{scope:?} {reason:?}"),
        code: Some(reason),
        scope: Some(scope),
        id: Some(id.to_string()),
    };
    let msg = WsMessage::event("error", serde_json::to_value(err).unwrap());
    conn_mgr
        .send_to(conn_id, &serde_json::to_string(&msg).unwrap_or_default())
        .await;
}

fn error_ack(reason: SocketErrorReason) -> Value {
    serde_json::to_value(VersionedUpdateResponse::Error {
        reason: Some(reason),
    })
    .unwrap()
}

fn field_value(field: &str, value: &Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert(field.to_string(), value.clone());
    Value::Object(m)
}

fn versioned_ack(result: &VersionedUpdateResult<Option<Value>>, field: &str) -> Value {
    let resp = match result {
        VersionedUpdateResult::Success { version, value } => VersionedUpdateResponse::Success {
            version: *version as f64,
            fields: field_value(field, value.as_ref().unwrap_or(&Value::Null)),
        },
        VersionedUpdateResult::VersionMismatch { version, value } => {
            VersionedUpdateResponse::VersionMismatch {
                version: *version as f64,
                fields: field_value(field, value.as_ref().unwrap_or(&Value::Null)),
            }
        }
        VersionedUpdateResult::Error => VersionedUpdateResponse::Error { reason: None },
    };
    serde_json::to_value(resp).unwrap()
}

/// Process an incoming WebSocket event from a CLI connection.
pub async fn handle_cli_event(
    conn_id: &str,
    namespace: &str,
    event: &str,
    data: Value,
    request_id: Option<&str>,
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
    let req: SessionMessageRequest = serde_json::from_value(data).ok()?;

    let session = match sync_engine
        .resolve_session_access(&req.sid, namespace)
        .await
    {
        Ok((_, session)) => session,
        Err(reason) => {
            emit_access_error(conn_mgr, conn_id, SocketErrorScope::Session, &req.sid, reason.into()).await;
            return None;
        }
    };
    let flavor = session.metadata.as_ref().and_then(|m| m.flavor);

    let content = if let Some(s) = req.message.as_str() {
        serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
    } else {
        req.message
    };

    let msg = match sync_engine.add_cli_message(
        &req.sid,
        namespace,
        flavor,
        &content,
        req.local_id.as_deref(),
    ) {
        Ok(m) => m,
        Err(_) => return None,
    };

    let broadcast = WsBroadcast::new(
        msg.seq,
        msg.created_at,
        &UpdateBody::NewMessage {
            sid: req.sid.clone(),
            message: UpdateMessage {
                id: msg.id,
                seq: msg.seq as f64,
                created_at: msg.created_at as f64,
                local_id: msg.local_id,
                content: msg.content.unwrap_or(Value::Null),
            },
        },
    );
    conn_mgr
        .broadcast_to_session(&req.sid, &broadcast.to_ws_string(), Some(conn_id))
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
    let req: SessionMessageDeltaRequest = match serde_json::from_value(data) {
        Ok(r) => r,
        Err(_) => return,
    };

    if let Err(reason) = sync_engine
        .resolve_session_access(&req.sid, namespace)
        .await
    {
        emit_access_error(conn_mgr, conn_id, SocketErrorScope::Session, &req.sid, reason.into()).await;
        return;
    }

    let now = now_millis();
    let broadcast = WsBroadcast::new(
        now,
        now,
        &UpdateBody::MsgDelta {
            sid: req.sid.clone(),
            delta: req.delta.clone(),
        },
    );
    conn_mgr
        .broadcast_to_session(&req.sid, &broadcast.to_ws_string(), Some(conn_id))
        .await;

    let delta_data = MessageDeltaData {
        message_id: req.delta.message_id,
        local_id: None,
        text: req.delta.text,
        is_final: req.delta.is_final,
        seq: None,
    };
    sync_engine
        .handle_realtime_event(SyncEvent::MessageDelta {
            session_id: req.sid,
            namespace: Some(namespace.to_string()),
            delta: delta_data,
        })
        .await;
}

async fn handle_update_metadata(
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_id: &str,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let req: SessionUpdateMetadataRequest = serde_json::from_value(data).ok()?;
    let metadata_value = serde_json::to_value(&req.metadata).ok()?;

    if let Err(reason) = sync_engine
        .resolve_session_access(&req.sid, namespace)
        .await
    {
        return Some(error_ack(reason.into()));
    }

    let result = sync_engine
        .update_session_metadata(&req.sid, &metadata_value, req.expected_version, namespace)
        .await;

    if let VersionedUpdateResult::Success { version, .. } = &result {
        let now = now_millis();
        let broadcast = WsBroadcast::new(
            now,
            now,
            &UpdateBody::UpdateSession {
                sid: req.sid.clone(),
                metadata: Some(VersionedValue {
                    version: *version as f64,
                    value: metadata_value,
                }),
                agent_state: None,
            },
        );
        conn_mgr
            .broadcast_to_session(&req.sid, &broadcast.to_ws_string(), Some(conn_id))
            .await;
    }

    Some(versioned_ack(&result, "metadata"))
}

async fn handle_update_state(
    namespace: &str,
    data: Value,
    sync_engine: &Arc<SyncEngine>,
    conn_id: &str,
    conn_mgr: &Arc<ConnectionManager>,
) -> Option<Value> {
    let req: SessionUpdateStateRequest = serde_json::from_value(data).ok()?;

    if let Err(reason) = sync_engine
        .resolve_session_access(&req.sid, namespace)
        .await
    {
        return Some(error_ack(reason.into()));
    }

    let result = sync_engine
        .update_session_agent_state(
            &req.sid,
            Some(&req.agent_state),
            req.expected_version,
            namespace,
        )
        .await;

    if let VersionedUpdateResult::Success { version, .. } = &result {
        let now = now_millis();
        let broadcast = WsBroadcast::new(
            now,
            now,
            &UpdateBody::UpdateSession {
                sid: req.sid.clone(),
                metadata: None,
                agent_state: Some(VersionedValue {
                    version: *version as f64,
                    value: req.agent_state.clone(),
                }),
            },
        );
        conn_mgr
            .broadcast_to_session(&req.sid, &broadcast.to_ws_string(), Some(conn_id))
            .await;
    }

    Some(versioned_ack(&result, "agentState"))
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

    if let Err(reason) = sync_engine
        .resolve_session_access(&req.sid, namespace)
        .await
    {
        emit_access_error(conn_mgr, conn_id, SocketErrorScope::Session, &req.sid, reason.into()).await;
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
        emit_access_error(conn_mgr, conn_id, SocketErrorScope::Session, &req.sid, reason.into()).await;
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
        emit_access_error(
            conn_mgr,
            conn_id,
            SocketErrorScope::Machine,
            &req.machine_id,
            SocketErrorReason::NotFound,
        )
        .await;
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
        return Some(error_ack(SocketErrorReason::NotFound));
    }

    let result = sync_engine
        .update_machine_metadata(mid, &metadata, req.expected_version, namespace)
        .await;

    if let VersionedUpdateResult::Success { version, .. } = &result {
        let now = now_millis();
        let broadcast = WsBroadcast::new(
            now,
            now,
            &UpdateBody::UpdateMachine {
                machine_id: mid.to_string(),
                metadata: Some(VersionedValue {
                    version: *version as f64,
                    value: metadata.clone(),
                }),
                runner_state: None,
            },
        );
        conn_mgr
            .broadcast_to_machine(mid, &broadcast.to_ws_string(), Some(conn_id))
            .await;
    }

    Some(versioned_ack(&result, "metadata"))
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
        return Some(error_ack(SocketErrorReason::NotFound));
    }

    let result = sync_engine
        .update_machine_runner_state(mid, Some(runner_state), req.expected_version, namespace)
        .await;

    if let VersionedUpdateResult::Success { version, .. } = &result {
        let now = now_millis();
        let broadcast = WsBroadcast::new(
            now,
            now,
            &UpdateBody::UpdateMachine {
                machine_id: mid.to_string(),
                metadata: None,
                runner_state: Some(VersionedValue {
                    version: *version as f64,
                    value: runner_state.clone(),
                }),
            },
        );
        conn_mgr
            .broadcast_to_machine(mid, &broadcast.to_ws_string(), Some(conn_id))
            .await;
    }

    Some(versioned_ack(&result, "runnerState"))
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
