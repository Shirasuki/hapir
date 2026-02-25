use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use tracing::{debug, info};

use hapir_infra::rpc::{RpcHandlerGroup, RpcRegistry};
use hapir_infra::utils::message_queue::MessageQueue2;
use hapir_infra::ws::session_client::WsSessionClient;

use hapir_shared::modes::SessionMode;

/// Transforms raw RPC params into the final message string.
/// Each agent can inject its own attachment handling logic here.
pub type MessagePreProcessor = Box<dyn Fn(&serde_json::Value) -> String + Send + Sync>;

/// Callback that applies config params to the agent's mode struct.
pub type ApplyConfigFn<Mode> = Box<dyn Fn(&mut Mode, &serde_json::Value) + Send + Sync>;

pub type OnKillFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Common RPC handlers shared by all agent flavors.
pub struct CommonRpc<Mode: Clone + Send + 'static> {
    queue: Arc<MessageQueue2<Mode>>,
    log_tag: &'static str,
    current_mode: Arc<Mutex<Mode>>,
    apply_config: Arc<ApplyConfigFn<Mode>>,
    on_kill: Option<OnKillFn>,
    switch_notify: Option<Arc<Notify>>,
    session_mode: Option<Arc<std::sync::Mutex<SessionMode>>>,
    pre_process: Option<Arc<MessagePreProcessor>>,
}

impl<Mode: Clone + Send + 'static> CommonRpc<Mode> {
    pub fn new(config: CommonRpcConfig<Mode>) -> Self {
        Self {
            queue: config.queue,
            log_tag: config.log_tag,
            current_mode: config.current_mode,
            apply_config: config.apply_config,
            on_kill: config.on_kill,
            switch_notify: config.switch_notify,
            session_mode: config.session_mode,
            pre_process: config.pre_process,
        }
    }
}

pub struct CommonRpcConfig<Mode: Clone + Send + 'static> {
    pub queue: Arc<MessageQueue2<Mode>>,
    pub log_tag: &'static str,
    pub current_mode: Arc<Mutex<Mode>>,
    pub apply_config: Arc<ApplyConfigFn<Mode>>,
    pub on_kill: Option<OnKillFn>,
    pub switch_notify: Option<Arc<Notify>>,
    pub session_mode: Option<Arc<std::sync::Mutex<SessionMode>>>,
    pub pre_process: Option<Arc<MessagePreProcessor>>,
}

impl<Mode: Clone + Send + 'static> RpcHandlerGroup<WsSessionClient> for CommonRpc<Mode> {
    fn register_rpc_handlers<'a>(
        &'a self,
        ws: &'a WsSessionClient,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            register_on_user_message(
                ws,
                self.queue.clone(),
                self.log_tag,
                self.current_mode.clone(),
                self.switch_notify.clone(),
                self.session_mode.clone(),
                self.pre_process.clone(),
            )
            .await;

            register_set_session_config(
                ws,
                self.log_tag,
                self.current_mode.clone(),
                self.apply_config.clone(),
            )
            .await;

            register_kill_session(ws, self.queue.clone(), self.log_tag, self.on_kill.clone()).await;
        })
    }
}

async fn register_on_user_message<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    queue: Arc<MessageQueue2<Mode>>,
    log_tag: &'static str,
    current_mode: Arc<Mutex<Mode>>,
    switch_notify: Option<Arc<Notify>>,
    session_mode: Option<Arc<std::sync::Mutex<SessionMode>>>,
    pre_process: Option<Arc<MessagePreProcessor>>,
) {
    ws.register_rpc("on-user-message", move |params| {
        let q = queue.clone();
        let mode = current_mode.clone();
        let switch = switch_notify.clone();
        let sm = session_mode.clone();
        let pp = pre_process.clone();
        async move {
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
        }
    })
    .await;
}

async fn register_set_session_config<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    log_tag: &'static str,
    current_mode: Arc<Mutex<Mode>>,
    apply_config: Arc<ApplyConfigFn<Mode>>,
) {
    ws.register_rpc("set-session-config", move |params| {
        let mode = current_mode.clone();
        let apply = apply_config.clone();
        async move {
            let mut m = mode.lock().await;
            apply(&mut m, &params);
            debug!("[{log_tag}] set-session-config applied");
            serde_json::json!({
                "ok": true,
                "applied": {
                    "permissionMode": params.get("permissionMode"),
                    "modelMode": params.get("modelMode"),
                }
            })
        }
    })
    .await;
}

async fn register_kill_session<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    queue: Arc<MessageQueue2<Mode>>,
    log_tag: &'static str,
    on_kill: Option<OnKillFn>,
) {
    ws.register_rpc("killSession", move |_params| {
        let q = queue.clone();
        let extra = on_kill.clone();
        async move {
            debug!("[{log_tag}] killSession RPC received");
            q.close().await;
            if let Some(f) = extra {
                f().await;
            }
            serde_json::json!({"ok": true})
        }
    })
    .await;
}

/// Register the `switch` RPC handler. Shared helper for agents that support mode switching.
pub async fn register_switch_handler(
    ws: &WsSessionClient,
    log_tag: &'static str,
    switch_notify: Arc<Notify>,
) {
    ws.register_rpc("switch", move |_params| {
        let notify = switch_notify.clone();
        async move {
            info!("[{log_tag}] switch RPC received, requesting mode switch");
            notify.notify_one();
            serde_json::json!({"ok": true})
        }
    })
    .await;
}

/// Register the `abort` RPC handler for ACP-backend agents. Shared helper.
pub async fn register_acp_abort_handler<Mode: Clone + Send + 'static>(
    ws: &WsSessionClient,
    log_tag: &'static str,
    backend: Arc<dyn hapir_acp::types::AgentBackend>,
    session_base: Arc<crate::agent::session_base::AgentSessionBase<Mode>>,
) {
    ws.register_rpc("abort", move |_params| {
        let b = backend.clone();
        let sb = session_base.clone();
        async move {
            debug!("[{log_tag}] abort RPC received");
            if let Some(sid) = sb.session_id.lock().await.clone() {
                let _ = b.cancel_prompt(&sid).await;
            }
            sb.on_thinking_change(false).await;
            serde_json::json!({"ok": true})
        }
    })
    .await;
}
