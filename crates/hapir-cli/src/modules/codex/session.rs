use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use tracing::{debug, info};

use crate::agent::session_base::AgentSessionBase;
use hapir_acp::codex_app_server::backend::CodexAppServerBackend;
use hapir_acp::types::{AgentBackend, PermissionResponse};
use hapir_infra::rpc::{RpcHandlerGroup, RpcRegistry};
use hapir_infra::ws::session_client::WsSessionClient;

use super::CodexMode;

pub struct CodexSession {
    pub base: Arc<AgentSessionBase<CodexMode>>,
    pub backend: Arc<CodexAppServerBackend>,
}

impl CodexSession {
    /// Forward permission requests from the ACP backend to agent state
    /// so the web UI can render approval buttons.
    pub fn setup_permission_forwarding(&self) {
        let ws = self.base.ws_client.clone();
        self.backend.on_permission_request(Box::new(
            move |req: hapir_acp::types::PermissionRequest| {
                let ws = ws.clone();
                let tool_name = req.kind.clone().unwrap_or_default();
                let arguments = req.raw_input.clone().unwrap_or(serde_json::json!({}));
                let key = req.id.clone();
                let requested_at = SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as f64;

                tokio::spawn(async move {
                    let _ = ws
                        .update_agent_state(move |mut state| {
                            if let Some(obj) = state.as_object_mut() {
                                let requests = obj
                                    .entry("requests")
                                    .or_insert_with(|| serde_json::json!({}));
                                if let Some(req_map) = requests.as_object_mut() {
                                    req_map.insert(
                                        key,
                                        serde_json::json!({
                                            "tool": tool_name,
                                            "arguments": arguments,
                                            "createdAt": requested_at,
                                        }),
                                    );
                                }
                            }
                            state
                        })
                        .await;
                });
            },
        ));
    }
}

impl RpcHandlerGroup<WsSessionClient> for CodexSession {
    fn register_rpc_handlers<'a>(
        &'a self,
        ws: &'a WsSessionClient,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let switch_notify = self.base.switch_notify.clone();
            ws.register_rpc("switch", move |_params| {
                let notify = switch_notify.clone();
                async move {
                    info!("[runCodex] switch RPC received, requesting mode switch");
                    notify.notify_one();
                    serde_json::json!({"ok": true})
                }
            })
            .await;

            let backend_for_abort = self.backend.clone();
            let base_for_abort = self.base.clone();
            ws.register_rpc("abort", move |_params| {
                let b = backend_for_abort.clone();
                let sb = base_for_abort.clone();
                async move {
                    debug!("[runCodex] abort RPC received");
                    if let Some(sid) = sb.session_id.lock().await.clone() {
                        let _ = b.cancel_prompt(&sid).await;
                    }
                    sb.on_thinking_change(false).await;
                    serde_json::json!({"ok": true})
                }
            })
            .await;

            let backend_for_perm = self.backend.clone();
            let base_for_perm = self.base.clone();
            ws.register_rpc("permission", move |params| {
                let b = backend_for_perm.clone();
                let sb = base_for_perm.clone();
                async move {
                    debug!("[runCodex] permission RPC received: {:?}", params);

                    let id = params
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let approved = params
                        .get("approved")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    let option_id = if approved { "accept" } else { "deny" };
                    let perm_req = hapir_acp::types::PermissionRequest {
                        id: id.clone(),
                        session_id: String::new(),
                        tool_call_id: id.clone(),
                        title: None,
                        kind: None,
                        raw_input: None,
                        raw_output: None,
                        options: vec![],
                    };
                    let perm_resp = PermissionResponse::Selected {
                        option_id: option_id.to_string(),
                    };

                    if let Some(sid) = sb.session_id.lock().await.clone() {
                        let _ = b.respond_to_permission(&sid, &perm_req, perm_resp).await;
                    } else {
                        let _ = b
                            .respond_to_permission(
                                "",
                                &perm_req,
                                PermissionResponse::Selected {
                                    option_id: option_id.to_string(),
                                },
                            )
                            .await;
                    }

                    let id_for_state = id.clone();
                    let status = if approved { "approved" } else { "denied" };
                    let completed_at = SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as f64;

                    let _ = sb
                        .ws_client
                        .update_agent_state(move |mut state| {
                            if let Some(requests) =
                                state.get_mut("requests").and_then(|v| v.as_object_mut())
                            {
                                if let Some(request) = requests.remove(&id_for_state) {
                                    if state.get("completedRequests").is_none() {
                                        state["completedRequests"] = serde_json::json!({});
                                    }
                                    if let Some(completed_map) = state
                                        .get_mut("completedRequests")
                                        .and_then(|v| v.as_object_mut())
                                    {
                                        let mut entry: serde_json::Map<String, serde_json::Value> =
                                            request.as_object().cloned().unwrap_or_default();
                                        entry.insert(
                                            "status".to_string(),
                                            serde_json::json!(status),
                                        );
                                        entry.insert(
                                            "completedAt".to_string(),
                                            serde_json::json!(completed_at),
                                        );
                                        completed_map
                                            .insert(id_for_state, serde_json::json!(entry));
                                    }
                                }
                            }
                            state
                        })
                        .await;

                    serde_json::json!({"ok": true})
                }
            })
            .await;
        })
    }
}
