use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use hapir_shared::schemas::{HapirSessionMetadata, Session};
use hapir_shared::socket::{
    MessageDelta, RpcRegisterRequest, SessionAliveRequest, SessionEndRequest,
    SessionMessageDeltaRequest, SessionMessageRequest, SessionUpdateMetadataRequest,
    SessionUpdateStateRequest, UpdateBody,
};
use hapir_shared::ws_protocol::WsBroadcast;

use super::client::{WsClient, WsClientConfig, WsClientType};
use crate::rpc::{self, RpcRegistry};
use crate::utils::time::epoch_ms;

pub struct WsSessionClient {
    ws: Arc<WsClient>,
    session_id: String,
    metadata: Arc<Mutex<Option<HapirSessionMetadata>>>,
    metadata_version: Arc<Mutex<i64>>,
    agent_state: Arc<Mutex<Option<Value>>>,
    agent_state_version: Arc<Mutex<i64>>,
    #[allow(dead_code)]
    last_seen_message_seq: Arc<Mutex<i64>>,
}

impl WsSessionClient {
    pub fn new(api_url: &str, token: &str, session: &Session) -> Self {
        let ws = Arc::new(WsClient::new(WsClientConfig {
            url: api_url.to_string(),
            auth_token: token.to_string(),
            client_type: WsClientType::Session,
            scope_id: session.id.clone(),
            max_reconnect_attempts: None,
        }));

        let agent_state_value = session
            .agent_state
            .as_ref()
            .and_then(|s| serde_json::to_value(s).ok());

        Self {
            ws,
            session_id: session.id.clone(),
            metadata: Arc::new(Mutex::new(session.metadata.clone())),
            metadata_version: Arc::new(Mutex::new(session.metadata_version as i64)),
            agent_state: Arc::new(Mutex::new(agent_state_value)),
            agent_state_version: Arc::new(Mutex::new(session.agent_state_version as i64)),
            last_seen_message_seq: Arc::new(Mutex::new(0)),
        }
    }

    pub async fn connect(&self, timeout: Duration) -> anyhow::Result<()> {
        self.register_update_handler().await;
        self.register_reconnect_handler().await;
        self.ws.connect(timeout).await?;
        Ok(())
    }

    async fn register_update_handler(&self) {
        let md = self.metadata.clone();
        let md_ver = self.metadata_version.clone();
        let as_ = self.agent_state.clone();
        let as_ver = self.agent_state_version.clone();
        let seq = self.last_seen_message_seq.clone();

        self.ws
            .on("update", move |data| {
                let md = md.clone();
                let md_ver = md_ver.clone();
                let as_ = as_.clone();
                let as_ver = as_ver.clone();
                let seq = seq.clone();

                tokio::spawn(async move {
                    let Ok(broadcast) = serde_json::from_value::<WsBroadcast>(data) else {
                        return;
                    };

                    {
                        let mut current = seq.lock().await;
                        if broadcast.seq > *current {
                            *current = broadcast.seq;
                        }
                    }

                    let Ok(UpdateBody::UpdateSession {
                        metadata,
                        agent_state,
                        ..
                    }) = serde_json::from_value::<UpdateBody>(broadcast.body)
                    else {
                        return;
                    };
                    if let Some(vv) = metadata {
                        let new_ver = vv.version as i64;
                        if new_ver > *md_ver.lock().await {
                            if let Ok(parsed) =
                                serde_json::from_value::<HapirSessionMetadata>(vv.value)
                            {
                                *md.lock().await = Some(parsed);
                                *md_ver.lock().await = new_ver;
                            }
                        }
                    }

                    if let Some(vv) = agent_state {
                        let new_ver = vv.version as i64;
                        if new_ver > *as_ver.lock().await {
                            *as_.lock().await = Some(vv.value);
                            *as_ver.lock().await = new_ver;
                        }
                    }
                });
            })
            .await;
    }

    async fn register_reconnect_handler(&self) {
        let ws = self.ws.clone();
        let sid = self.session_id.clone();

        self.ws
            .on_connect(move || {
                let ws = ws.clone();
                let sid = sid.clone();
                tokio::spawn(async move {
                    let req = SessionAliveRequest {
                        sid,
                        time: epoch_ms() as i64,
                        thinking: false,
                        thinking_status: None,
                        mode: "local".to_string(),
                        runtime: String::new(),
                    };
                    ws.emit(
                        "session-alive",
                        serde_json::to_value(&req).unwrap_or_default(),
                    )
                    .await;
                });
            })
            .await;
    }

    pub async fn keep_alive(
        &self,
        thinking: bool,
        thinking_status: Option<&str>,
        mode: &str,
        runtime: &str,
    ) {
        let req = SessionAliveRequest {
            sid: self.session_id.clone(),
            time: epoch_ms() as i64,
            thinking,
            thinking_status: thinking_status.map(|s| s.to_string()),
            mode: mode.to_string(),
            runtime: runtime.to_string(),
        };
        self.ws
            .emit(
                "session-alive",
                serde_json::to_value(&req).unwrap_or_default(),
            )
            .await;
    }

    pub async fn send_session_end(&self) {
        let req = SessionEndRequest {
            sid: self.session_id.clone(),
            time: epoch_ms() as i64,
        };
        self.ws
            .emit(
                "session-end",
                serde_json::to_value(&req).unwrap_or_default(),
            )
            .await;
    }

    pub async fn update_metadata<F>(&self, handler: F) -> anyhow::Result<()>
    where
        F: FnOnce(HapirSessionMetadata) -> HapirSessionMetadata,
    {
        let current = match self.metadata.lock().await.clone() {
            Some(m) => m,
            None => anyhow::bail!("no metadata to update"),
        };
        let updated = handler(current);
        let version = *self.metadata_version.lock().await;

        let req = SessionUpdateMetadataRequest {
            sid: self.session_id.clone(),
            expected_version: version,
            metadata: updated,
        };

        let ack = self
            .ws
            .emit_with_ack("update-metadata", serde_json::to_value(&req)?)
            .await?;

        apply_versioned_ack(&ack, "metadata", &self.metadata, &self.metadata_version).await;
        Ok(())
    }

    pub async fn update_agent_state<F>(&self, handler: F) -> anyhow::Result<()>
    where
        F: FnOnce(Value) -> Value,
    {
        let current = self
            .agent_state
            .lock()
            .await
            .clone()
            .unwrap_or(Value::Object(Default::default()));
        let updated = handler(current);
        let version = *self.agent_state_version.lock().await;

        let req = SessionUpdateStateRequest {
            sid: self.session_id.clone(),
            expected_version: version,
            agent_state: updated,
        };

        let ack = self
            .ws
            .emit_with_ack("update-state", serde_json::to_value(&req)?)
            .await?;

        apply_versioned_ack(
            &ack,
            "agentState",
            &self.agent_state,
            &self.agent_state_version,
        )
        .await;
        Ok(())
    }

    pub async fn send_message(&self, body: Value) {
        let req = SessionMessageRequest {
            sid: self.session_id.clone(),
            message: body,
        };
        self.ws
            .emit("message", serde_json::to_value(&req).unwrap_or_default())
            .await;
    }

    pub async fn send_message_delta(&self, message_id: &str, text: &str, is_final: bool) {
        let req = SessionMessageDeltaRequest {
            sid: self.session_id.clone(),
            delta: MessageDelta {
                message_id: message_id.to_string(),
                text: text.to_string(),
                is_final,
            },
        };
        self.ws
            .emit(
                "message-delta",
                serde_json::to_value(&req).unwrap_or_default(),
            )
            .await;
    }

    pub async fn register_rpc(
        &self,
        method: &str,
        handler: impl Fn(Value) -> Pin<Box<dyn Future<Output = Value> + Send>> + Send + Sync + 'static,
    ) {
        let scoped = rpc::scoped_method(&self.session_id, method);
        info!(method = %scoped, "registering session-scoped RPC handler");
        self.ws.register_rpc(&scoped, handler).await;
        self.ws
            .emit(
                "rpc-register",
                serde_json::to_value(&RpcRegisterRequest { method: scoped }).unwrap_or_default(),
            )
            .await;
    }

    pub async fn on(
        &self,
        event: impl Into<String>,
        handler: impl Fn(Value) + Send + Sync + 'static,
    ) {
        self.ws.on(event, handler).await;
    }

    pub async fn emit(&self, event: impl Into<String>, data: Value) {
        self.ws.emit(event, data).await;
    }

    pub async fn close(&self) {
        self.ws.close().await;
    }

    #[allow(dead_code)]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[allow(dead_code)]
    pub async fn metadata(&self) -> Option<HapirSessionMetadata> {
        self.metadata.lock().await.clone()
    }
}

impl RpcRegistry for WsSessionClient {
    fn register<F, Fut>(&self, method: &str, handler: F) -> impl Future<Output = ()> + Send
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Value> + Send + 'static,
    {
        let scoped = rpc::scoped_method(&self.session_id, method);
        let ws = self.ws.clone();
        let boxed_handler = move |params: Value| -> Pin<Box<dyn Future<Output = Value> + Send>> {
            Box::pin(handler(params))
        };
        async move {
            info!(method = %scoped, "registering session-scoped RPC handler");
            ws.register_rpc(&scoped, boxed_handler).await;
            ws.emit(
                "rpc-register",
                serde_json::to_value(&RpcRegisterRequest { method: scoped }).unwrap_or_default(),
            )
            .await;
        }
    }
}

async fn apply_versioned_ack<T: serde::de::DeserializeOwned + Clone>(
    ack: &Value,
    value_key: &str,
    local_value: &Mutex<Option<T>>,
    local_version: &Mutex<i64>,
) {
    let result = ack.get("result").and_then(|v| v.as_str()).unwrap_or("");

    match result {
        "success" | "version-mismatch" => {
            if let Some(ver) = ack.get("version").and_then(|v| v.as_i64()) {
                *local_version.lock().await = ver;
            }
            if let Some(val) = ack.get(value_key)
                && let Ok(parsed) = serde_json::from_value::<T>(val.clone())
            {
                *local_value.lock().await = Some(parsed);
            }
            if result == "version-mismatch" {
                debug!("version mismatch, local state updated from server");
            }
        }
        "error" => {
            let reason = ack
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            warn!(reason = reason, "versioned update error");
        }
        _ => {
            warn!(result = result, "unknown ack result");
        }
    }
}
