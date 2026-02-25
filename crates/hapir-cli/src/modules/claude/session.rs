use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;
use crate::agent::common_rpc::register_switch_handler;
use crate::agent::session_base::AgentSessionBase;
use hapir_infra::rpc::{RpcHandlerGroup, RpcRegistry};
use hapir_infra::ws::session_client::WsSessionClient;
use hapir_shared::modes::SessionMode;
use hapir_shared::schemas::SessionStartedBy;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::debug;

type PermissionResponseSender = tokio::sync::oneshot::Sender<(bool, Option<serde_json::Value>)>;

/// Claude-specific session extending AgentSessionBase.
///
/// Holds Claude-specific fields like env vars, CLI args, MCP server
/// configuration, and hook settings path.
pub struct ClaudeSession<Mode: Clone + Send + 'static> {
    pub base: Arc<AgentSessionBase<Mode>>,
    pub claude_env_vars: Option<HashMap<String, String>>,
    pub claude_args: Mutex<Option<Vec<String>>>,
    pub mcp_servers: HashMap<String, Value>,
    pub hook_settings_path: String,
    pub started_by: SessionStartedBy,
    pub starting_mode: SessionMode,
    pub local_launch_failure: Mutex<Option<LocalLaunchFailure>>,
    pub pending_permissions: Arc<Mutex<HashMap<String, PermissionResponseSender>>>,
    pub active_pid: Arc<AtomicU32>,
}

#[derive(Debug, Clone)]
pub struct LocalLaunchFailure {
    pub message: String,
    pub exit_reason: String,
}

impl<Mode: Clone + Send + 'static> ClaudeSession<Mode> {
    pub fn record_local_launch_failure(&self, message: String, exit_reason: String) {
        *self.local_launch_failure.blocking_lock() = Some(LocalLaunchFailure {
            message,
            exit_reason,
        });
    }

    /// Clear the current session ID (used by /clear command).
    pub async fn clear_session_id(&self) {
        *self.base.session_id.lock().await = None;
        debug!("[Session] Session ID cleared");
    }

    /// Consume one-time Claude flags from claude_args after Claude spawn.
    /// Currently handles: --resume (with or without session ID).
    pub async fn consume_one_time_flags(&self) {
        let mut args_guard = self.claude_args.lock().await;
        let args = match args_guard.as_mut() {
            Some(a) => a,
            None => return,
        };

        let mut filtered = Vec::new();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--resume" {
                // Check if next arg looks like a UUID
                if i + 1 < args.len() {
                    let next = &args[i + 1];
                    if !next.starts_with('-') && next.contains('-') {
                        debug!("[Session] Consumed --resume flag with session ID: {}", next);
                        i += 2;
                        continue;
                    }
                }
                debug!("[Session] Consumed --resume flag (no session ID)");
                i += 1;
                continue;
            }
            filtered.push(args[i].clone());
            i += 1;
        }

        if filtered.is_empty() {
            *args_guard = None;
        } else {
            *args_guard = Some(filtered);
        }
        debug!("[Session] Consumed one-time flags");
    }
}

impl<Mode: Clone + Send + 'static> RpcHandlerGroup<WsSessionClient> for ClaudeSession<Mode> {
    fn register_rpc_handlers<'a>(
        &'a self,
        ws: &'a WsSessionClient,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            register_switch_handler(ws, "runClaude", self.base.switch_notify.clone()).await;

            let pending = self.pending_permissions.clone();
            let base_ws = self.base.ws_client.clone();
            ws.register_rpc("permission", move |params| {
                let pending = pending.clone();
                let ws = base_ws.clone();
                async move {
                    debug!("[runClaude] permission RPC received: {:?}", params);

                    let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let approved = params
                        .get("approved")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let updated_input = params.get("updatedInput").cloned();

                    if let Some(tx) = pending.lock().await.remove(id) {
                        let _ = tx.send((approved, updated_input));

                        let id_clone = id.to_string();
                        let status = if approved { "approved" } else { "denied" };
                        let completed_at = SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as f64;

                        let _ = ws
                            .update_agent_state(move |mut state| {
                                if let Some(requests) =
                                    state.get_mut("requests").and_then(|v| v.as_object_mut())
                                    && let Some(request) = requests.remove(&id_clone)
                                {
                                    let completed_requests = state
                                        .get_mut("completedRequests")
                                        .and_then(|v| v.as_object_mut())
                                        .map(|obj| obj.clone())
                                        .unwrap_or_default();

                                    let mut updated_completed = completed_requests.clone();
                                    let mut completed_request =
                                        request.as_object().cloned().unwrap_or_default();
                                    completed_request
                                        .insert("status".to_string(), serde_json::json!(status));
                                    completed_request.insert(
                                        "completedAt".to_string(),
                                        serde_json::json!(completed_at),
                                    );

                                    updated_completed.insert(
                                        id_clone.clone(),
                                        serde_json::json!(completed_request),
                                    );
                                    state["completedRequests"] =
                                        serde_json::json!(updated_completed);
                                }
                                state
                            })
                            .await;
                    }

                    serde_json::json!({"ok": true})
                }
            })
            .await;

            let active_pid = self.active_pid.clone();
            let pending_for_abort = self.pending_permissions.clone();
            let base_for_abort = self.base.clone();
            ws.register_rpc("abort", move |_params| {
                let pid = active_pid.clone();
                let pending = pending_for_abort.clone();
                let base = base_for_abort.clone();
                async move {
                    debug!("[runClaude] abort RPC received");
                    let p = pid.load(Ordering::Relaxed);
                    if p != 0 {
                        debug!("[runClaude] Terminating PID {}", p);
                        #[cfg(unix)]
                        unsafe {
                            libc::kill(p as i32, libc::SIGTERM);
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = std::process::Command::new("taskkill")
                                .args(["/PID", &p.to_string(), "/T"])
                                .spawn();
                        }
                    }
                    pending.lock().await.clear();
                    base.on_thinking_change(false).await;
                    serde_json::json!({"ok": true})
                }
            })
            .await;
        })
    }
}
