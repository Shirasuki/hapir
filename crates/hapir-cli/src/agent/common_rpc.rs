use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use tracing::{debug, info};

use hapir_infra::utils::message_queue::MessageQueue2;
use hapir_infra::ws::session_client::WsSessionClient;

use hapir_shared::modes::SessionMode;

use crate::agent::session_base::AgentSessionBase;

/// Transforms raw RPC params into the final message string.
/// Each agent can inject its own attachment handling logic here.
pub type MessagePreProcessor = Box<dyn Fn(&serde_json::Value) -> String + Send + Sync>;

/// Register the `on-user-message` RPC handler.
///
/// When `switch_notify` and `session_mode` are provided, a web message
/// received while in local mode triggers a switch to remote.
pub async fn register_on_user_message_rpc<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    queue: Arc<MessageQueue2<Mode>>,
    current_mode: Arc<Mutex<Mode>>,
    switch_notify: Option<Arc<Notify>>,
    session_mode: Option<Arc<std::sync::Mutex<SessionMode>>>,
    log_tag: &'static str,
    pre_process: Option<Arc<MessagePreProcessor>>,
) {
    ws.register_rpc("on-user-message", move |params| {
        let q = queue.clone();
        let mode = current_mode.clone();
        let switch = switch_notify.clone();
        let sm = session_mode.clone();
        let pp = pre_process.clone();
        Box::pin(async move {
            let raw_text = params
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let message = match pp {
                Some(ref f) => f(&params),
                None => raw_text,
            };

            if message.is_empty() {
                return serde_json::json!({"ok": false, "reason": "empty message"});
            }

            let current = mode.lock().await.clone();

            let trimmed = message.trim();
            if trimmed == "/compact" || trimmed == "/clear" {
                debug!("[{log_tag}] Received {trimmed} command, isolate-and-clear");
                q.push_isolate_and_clear(message, current).await;
            } else {
                q.push(message, current).await;
            }

            if let (Some(switch_notify), Some(session_mode)) = (&switch, &sm) {
                let is_local = *session_mode.lock().unwrap() == SessionMode::Local;
                if is_local {
                    info!(
                        "[{log_tag}] Local mode: web message received, requesting switch to remote"
                    );
                    switch_notify.notify_one();
                }
            }

            serde_json::json!({"ok": true})
        })
    })
    .await;
}

/// Callback that applies config params to the agent's mode struct.
pub type ApplyConfigFn<Mode> = Box<dyn Fn(&mut Mode, &serde_json::Value) + Send + Sync>;

/// Register the `set-session-config` RPC handler.
///
/// `apply_config` maps incoming JSON params to the agent's mode struct fields.
pub async fn register_set_session_config_rpc<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    current_mode: Arc<Mutex<Mode>>,
    apply_config: Arc<ApplyConfigFn<Mode>>,
    log_tag: &'static str,
) {
    ws.register_rpc("set-session-config", move |params| {
        let mode = current_mode.clone();
        let apply = apply_config.clone();
        Box::pin(async move {
            let mut m = mode.lock().await;
            apply(&mut m, &params);
            debug!("[{log_tag}] set-session-config applied");
            serde_json::json!({"ok": true})
        })
    })
    .await;
}

pub type OnKillFn =
    Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// Register the `killSession` RPC handler.
///
/// Closes the queue and optionally runs `on_kill` (e.g. to disconnect an ACP backend).
pub async fn register_kill_session_rpc<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    queue: Arc<MessageQueue2<Mode>>,
    on_kill: Option<OnKillFn>,
    log_tag: &'static str,
) {
    ws.register_rpc("killSession", move |_params| {
        let q = queue.clone();
        let extra = on_kill.clone();
        Box::pin(async move {
            debug!("[{log_tag}] killSession RPC received");
            q.close().await;
            if let Some(f) = extra {
                f().await;
            }
            serde_json::json!({"ok": true})
        })
    })
    .await;
}

/// Register the `switch` RPC handler.
pub async fn register_switch_rpc(
    ws: &WsSessionClient,
    switch_notify: Arc<Notify>,
    log_tag: &'static str,
) {
    ws.register_rpc("switch", move |_params| {
        let notify = switch_notify.clone();
        Box::pin(async move {
            info!("[{log_tag}] switch RPC received, requesting mode switch");
            notify.notify_one();
            serde_json::json!({"ok": true})
        })
    })
    .await;
}

/// Register the `abort` RPC handler for ACP-backend agents.
///
/// Cancels the active prompt and resets thinking state.
pub async fn register_acp_abort_rpc<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    backend: Arc<dyn hapir_acp::types::AgentBackend>,
    session_base: Arc<AgentSessionBase<Mode>>,
    log_tag: &'static str,
) {
    ws.register_rpc("abort", move |_params| {
        let b = backend.clone();
        let sb = session_base.clone();
        Box::pin(async move {
            debug!("[{log_tag}] abort RPC received");
            if let Some(sid) = sb.session_id.lock().await.clone() {
                let _ = b.cancel_prompt(&sid).await;
            }
            sb.on_thinking_change(false).await;
            serde_json::json!({"ok": true})
        })
    })
    .await;
}
