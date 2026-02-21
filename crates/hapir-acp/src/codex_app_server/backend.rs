use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::codex_app_server::sender_ex::ArcMutexSender;
use crate::transport::AcpStdioTransport;
use crate::types::{
    AgentBackend, AgentSessionConfig, OnPermissionRequestFn, OnUpdateFn, PermissionRequest,
    PermissionResponse, PromptContent,
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::debug;

use super::message_handler::CodexMessageHandler;

struct PendingPermission {
    tx: oneshot::Sender<Value>,
}

/// Codex App Server backend using `codex app-server` over JSON-RPC 2.0 stdio.
///
/// Method mapping vs ACP SDK:
/// - `initialize` + `initialized` notification (two-step handshake)
/// - `thread/start` instead of `session/new`
/// - `turn/start` instead of `session/prompt`
/// - `turn/interrupt` instead of `session/cancel`
/// - Permission requests via `item/commandExecution/requestApproval`
///   and `item/fileChange/requestApproval`
pub struct CodexAppServerBackend {
    command: String,
    args: Vec<String>,
    env: Option<HashMap<String, String>>,
    transport: Mutex<Option<Arc<AcpStdioTransport>>>,
    permission_handler: Mutex<Option<OnPermissionRequestFn>>,
    pending_permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    active_thread_id: Mutex<Option<String>>,
    notification_tx: ArcMutexSender<(String, Value)>,
}

impl CodexAppServerBackend {
    pub fn new(command: String, args: Vec<String>, env: Option<HashMap<String, String>>) -> Self {
        Self {
            command,
            args,
            env,
            transport: Mutex::new(None),
            permission_handler: Mutex::new(None),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            active_thread_id: Mutex::new(None),
            notification_tx: ArcMutexSender::new(),
        }
    }
}

impl AgentBackend for CodexAppServerBackend {
    fn initialize(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.transport.lock().await.is_some() {
                return Ok(());
            }

            let transport = AcpStdioTransport::new(&self.command, &self.args, self.env.clone())
                .map_err(|e| anyhow::anyhow!(e))?;

            let notif_tx = self.notification_tx.clone();
            transport
                .on_notification(move |method, params| {
                    notif_tx.send((method, params));
                })
                .await;

            *self.transport.lock().await = Some(transport.clone());

            let pending_cmd = self.pending_permissions.clone();
            transport
                .register_request_handler(
                    "item/commandExecution/requestApproval",
                    Arc::new(move |params: Value, _id: Value| {
                        let pending = pending_cmd.clone();
                        Box::pin(async move {
                            let id = params
                                .as_object()
                                .and_then(|o| o.get("itemId").or_else(|| o.get("id")))
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool-unknown")
                                .to_string();

                            let (tx, rx) = oneshot::channel();
                            pending.lock().await.insert(id, PendingPermission { tx });
                            rx.await.unwrap_or_else(|_| serde_json::json!("decline"))
                        })
                    }),
                )
                .await;

            let pending_file = self.pending_permissions.clone();
            transport
                .register_request_handler(
                    "item/fileChange/requestApproval",
                    Arc::new(move |params: Value, _id: Value| {
                        let pending = pending_file.clone();
                        Box::pin(async move {
                            let id = params
                                .as_object()
                                .and_then(|o| o.get("itemId").or_else(|| o.get("id")))
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool-unknown")
                                .to_string();

                            let (tx, rx) = oneshot::channel();
                            pending.lock().await.insert(id, PendingPermission { tx });
                            rx.await.unwrap_or_else(|_| serde_json::json!("decline"))
                        })
                    }),
                )
                .await;

            let response = transport
                .send_request_default(
                    "initialize",
                    serde_json::json!({
                        "protocolVersion": "2025-03-26",
                        "clientInfo": {
                            "name": "hapir",
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "capabilities": {}
                    }),
                )
                .await
                .map_err(|e| anyhow::anyhow!(e))?;

            debug!("[CodexAppServer] Initialize response: {:?}", response);

            transport
                .send_notification("initialized", serde_json::json!({}))
                .await;

            debug!("[CodexAppServer] Initialized");
            Ok(())
        })
    }

    fn new_session(
        &self,
        config: AgentSessionConfig,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + '_>> {
        Box::pin(async move {
            let transport = self
                .transport
                .lock()
                .await
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Codex transport not initialized"))?;

            let response = transport
                .send_request_default(
                    "thread/start",
                    serde_json::json!({
                        "cwd": config.cwd,
                    }),
                )
                .await
                .map_err(|e| anyhow::anyhow!(e))?;

            debug!("[CodexAppServer] thread/start response: {:?}", response);

            let thread_id = response
                .as_object()
                .and_then(|o| {
                    o.get("threadId")
                        .or_else(|| o.get("id"))
                        .or_else(|| o.get("thread").and_then(|t| t.get("id")))
                })
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid thread/start response: {response}"))?
                .to_string();

            *self.active_thread_id.lock().await = Some(thread_id.clone());
            debug!("[CodexAppServer] Thread started: {}", thread_id);
            Ok(thread_id)
        })
    }

    fn prompt(
        &self,
        session_id: &str,
        content: Vec<PromptContent>,
        on_update: OnUpdateFn,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        let session_id = session_id.to_string();
        let content = content.clone();
        Box::pin(async move {
            let transport = self
                .transport
                .lock()
                .await
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Codex transport not initialized"))?;

            // Create per-prompt channels
            let (ntx, mut nrx) = mpsc::unbounded_channel::<(String, Value)>();
            self.notification_tx.set(Some(ntx));

            let input: Vec<Value> = content
                .iter()
                .map(|c| match c {
                    PromptContent::Text { text } => serde_json::json!({
                        "type": "text",
                        "text": text,
                    }),
                })
                .collect();

            // Spawn notification processor: owns the message handler,
            // processes all notifications, breaks on turn/completed.
            let process_handle = tokio::spawn(async move {
                let mut handler = CodexMessageHandler::new(move |msg| {
                    on_update(msg);
                });
                loop {
                    match nrx.recv().await {
                        Some((method, params)) => {
                            if handler.handle_notification(&method, &params) {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            });

            // Send turn/start and wait for the RPC ack (comes back immediately
            // with status "inProgress"). Use default timeout for the ack only.
            let rpc_result = transport
                .send_request_default(
                    "turn/start",
                    serde_json::json!({
                        "threadId": session_id,
                        "input": input,
                    }),
                )
                .await;

            if let Err(e) = rpc_result {
                process_handle.abort();
                self.notification_tx.set(None);
                return Err(anyhow::anyhow!(e));
            }

            // Turn accepted, wait for turn/completed via notification processor
            let _ = process_handle.await;

            // Close channels
            self.notification_tx.set(None);

            Ok(())
        })
    }

    fn cancel_prompt(
        &self,
        session_id: &str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        let session_id = session_id.to_string();
        Box::pin(async move {
            if let Some(transport) = self.transport.lock().await.as_ref() {
                let _ = transport
                    .send_request_default(
                        "turn/interrupt",
                        serde_json::json!({"threadId": session_id}),
                    )
                    .await;
            }
            Ok(())
        })
    }

    fn respond_to_permission(
        &self,
        _session_id: &str,
        request: &PermissionRequest,
        response: PermissionResponse,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        let request_id = request.id.clone();
        Box::pin(async move {
            let pending = self.pending_permissions.lock().await.remove(&request_id);
            if let Some(p) = pending {
                let result = match response {
                    PermissionResponse::Selected { ref option_id } if option_id == "deny" => {
                        serde_json::json!("decline")
                    }
                    PermissionResponse::Selected { ref option_id }
                        if option_id == "accept_session" =>
                    {
                        serde_json::json!("acceptForSession")
                    }
                    PermissionResponse::Selected { .. } => serde_json::json!("accept"),
                    _ => serde_json::json!("decline"),
                };
                let _ = p.tx.send(result);
            } else {
                debug!(
                    "[CodexAppServer] No pending permission for id {}",
                    request_id
                );
            }
            Ok(())
        })
    }

    fn on_permission_request(&self, handler: OnPermissionRequestFn) {
        let mut h = self.permission_handler.blocking_lock();
        *h = Some(handler);
    }

    fn disconnect(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(transport) = self.transport.lock().await.take() {
                transport.close().await;
            }
            Ok(())
        })
    }
}
