use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use hapir_shared::schemas::{HapirMachineMetadata, MachineRunnerState};
use hapir_shared::socket::{
    MachineAliveRequest, MachineUpdateMetadataRequest, MachineUpdateStateRequest,
    RpcRegisterRequest, SessionEndRequest, UpdateBody,
};
use hapir_shared::ws_protocol::WsBroadcast;
use serde_json::Value;
use tokio::sync::Mutex;

use super::client::{WsClient, WsClientConfig, WsClientType};
use crate::rpc::{self, RpcRegistry};
use crate::utils::time::epoch_ms;

pub struct WsMachineClient {
    ws: Arc<WsClient>,
    machine_id: String,
    metadata: Arc<Mutex<Option<HapirMachineMetadata>>>,
    metadata_version: Arc<Mutex<i64>>,
    runner_state: Arc<Mutex<Option<MachineRunnerState>>>,
    runner_state_version: Arc<Mutex<i64>>,
    keep_alive_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl WsMachineClient {
    pub fn new(api_url: &str, token: &str, machine_id: &str) -> Self {
        let ws = Arc::new(WsClient::new(WsClientConfig {
            url: api_url.to_string(),
            auth_token: token.to_string(),
            client_type: WsClientType::Machine,
            scope_id: machine_id.to_string(),
            max_reconnect_attempts: None,
        }));

        Self {
            ws,
            machine_id: machine_id.to_string(),
            metadata: Arc::new(Mutex::new(None)),
            metadata_version: Arc::new(Mutex::new(0)),
            runner_state: Arc::new(Mutex::new(None)),
            runner_state_version: Arc::new(Mutex::new(0)),
            keep_alive_handle: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn connect(&self, timeout: Duration) -> anyhow::Result<()> {
        self.register_update_handler().await;
        self.register_reconnect_handler().await;
        self.ws.connect(timeout).await?;
        self.start_keep_alive().await;
        Ok(())
    }

    async fn register_update_handler(&self) {
        let md = self.metadata.clone();
        let md_ver = self.metadata_version.clone();
        let rs = self.runner_state.clone();
        let rs_ver = self.runner_state_version.clone();

        self.ws
            .on("update", move |data| {
                let md = md.clone();
                let md_ver = md_ver.clone();
                let rs = rs.clone();
                let rs_ver = rs_ver.clone();

                tokio::spawn(async move {
                    let Ok(broadcast) = serde_json::from_value::<WsBroadcast>(data) else {
                        return;
                    };
                    let Ok(UpdateBody::UpdateMachine {
                        metadata,
                        runner_state,
                        ..
                    }) = serde_json::from_value::<UpdateBody>(broadcast.body)
                    else {
                        return;
                    };

                    if let Some(vv) = metadata {
                        let new_ver = vv.version as i64;
                        if new_ver > *md_ver.lock().await {
                            if let Ok(parsed) =
                                serde_json::from_value::<HapirMachineMetadata>(vv.value)
                            {
                                *md.lock().await = Some(parsed);
                                *md_ver.lock().await = new_ver;
                            }
                        }
                    }

                    if let Some(vv) = runner_state {
                        let new_ver = vv.version as i64;
                        if new_ver > *rs_ver.lock().await {
                            if let Ok(parsed) =
                                serde_json::from_value::<MachineRunnerState>(vv.value)
                            {
                                *rs.lock().await = Some(parsed);
                                *rs_ver.lock().await = new_ver;
                            }
                        }
                    }
                });
            })
            .await;
    }

    async fn register_reconnect_handler(&self) {
        let ws = self.ws.clone();
        let mid = self.machine_id.clone();
        let md = self.metadata.clone();
        let md_ver = self.metadata_version.clone();
        let rs = self.runner_state.clone();
        let rs_ver = self.runner_state_version.clone();

        self.ws
            .on_connect(move || {
                let ws = ws.clone();
                let mid = mid.clone();
                let md = md.clone();
                let md_ver = md_ver.clone();
                let rs = rs.clone();
                let rs_ver = rs_ver.clone();
                tokio::spawn(async move {
                    let md_snapshot = md.lock().await.clone();
                    if let Some(metadata) = md_snapshot {
                        let version = *md_ver.lock().await;
                        let req = MachineUpdateMetadataRequest {
                            machine_id: mid.clone(),
                            expected_version: version,
                            metadata,
                        };
                        let _ = ws
                            .emit_with_ack(
                                "machine-update-metadata",
                                serde_json::to_value(&req).unwrap_or_default(),
                            )
                            .await;
                    }
                    let rs_snapshot = rs.lock().await.clone();
                    if let Some(state) = rs_snapshot {
                        let version = *rs_ver.lock().await;
                        let req = MachineUpdateStateRequest {
                            machine_id: mid.clone(),
                            expected_version: version,
                            runner_state: serde_json::to_value(&state).unwrap_or_default(),
                        };
                        let _ = ws
                            .emit_with_ack(
                                "machine-update-state",
                                serde_json::to_value(&req).unwrap_or_default(),
                            )
                            .await;
                    }
                });
            })
            .await;
    }

    async fn start_keep_alive(&self) {
        let ws = self.ws.clone();
        let mid = self.machine_id.clone();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(20));
            loop {
                interval.tick().await;
                let time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                let req = MachineAliveRequest {
                    machine_id: mid.clone(),
                    time,
                };
                ws.emit(
                    "machine-alive",
                    serde_json::to_value(&req).unwrap_or_default(),
                )
                .await;
            }
        });
        *self.keep_alive_handle.lock().await = Some(handle);
    }

    pub async fn update_metadata<F>(&self, handler: F) -> anyhow::Result<()>
    where
        F: FnOnce(HapirMachineMetadata) -> HapirMachineMetadata,
    {
        let current = self.metadata.lock().await.clone().unwrap_or_default();
        let updated = handler(current);
        let version = *self.metadata_version.lock().await;

        let req = MachineUpdateMetadataRequest {
            machine_id: self.machine_id.clone(),
            expected_version: version,
            metadata: updated,
        };

        let ack = self
            .ws
            .emit_with_ack("machine-update-metadata", serde_json::to_value(&req)?)
            .await?;

        if let Some(ver) = ack.get("version").and_then(|v| v.as_i64()) {
            *self.metadata_version.lock().await = ver;
        }
        if let Some(val) = ack.get("metadata")
            && let Ok(parsed) = serde_json::from_value::<HapirMachineMetadata>(val.clone())
        {
            *self.metadata.lock().await = Some(parsed);
        }
        Ok(())
    }

    pub async fn update_runner_state<F>(&self, handler: F) -> anyhow::Result<()>
    where
        F: FnOnce(MachineRunnerState) -> MachineRunnerState,
    {
        let default = MachineRunnerState {
            status: hapir_shared::schemas::MachineRunnerStatus::Offline,
            pid: 0,
            http_port: 0,
            started_at: 0,
            shutdown_requested_at: None,
            shutdown_source: None,
        };
        let current = self.runner_state.lock().await.clone().unwrap_or(default);
        let updated = handler(current);
        let updated_value = serde_json::to_value(&updated)?;
        let version = *self.runner_state_version.lock().await;

        let req = MachineUpdateStateRequest {
            machine_id: self.machine_id.clone(),
            expected_version: version,
            runner_state: updated_value,
        };

        let ack = self
            .ws
            .emit_with_ack("machine-update-state", serde_json::to_value(&req)?)
            .await?;

        if let Some(ver) = ack.get("version").and_then(|v| v.as_i64()) {
            *self.runner_state_version.lock().await = ver;
        }
        if let Some(val) = ack.get("runnerState")
            && let Ok(parsed) = serde_json::from_value::<MachineRunnerState>(val.clone())
        {
            *self.runner_state.lock().await = Some(parsed);
        }
        Ok(())
    }

    pub async fn send_session_end(&self, session_id: &str) {
        let req = SessionEndRequest {
            sid: session_id.to_string(),
            time: epoch_ms() as i64,
        };
        self.ws
            .emit(
                "session-end",
                serde_json::to_value(&req).unwrap_or_default(),
            )
            .await;
    }

    pub async fn shutdown(&self) {
        if let Some(handle) = self.keep_alive_handle.lock().await.take() {
            handle.abort();
        }
        self.ws.close().await;
    }

    #[allow(dead_code)]
    pub fn machine_id(&self) -> &str {
        &self.machine_id
    }
}

impl RpcRegistry for WsMachineClient {
    fn register<F, Fut>(&self, method: &str, handler: F) -> impl Future<Output = ()> + Send
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Value> + Send + 'static,
    {
        let scoped = rpc::scoped_method(&self.machine_id, method);
        let ws = self.ws.clone();
        let boxed_handler = move |params: Value| -> Pin<Box<dyn Future<Output = Value> + Send>> {
            Box::pin(handler(params))
        };
        async move {
            ws.register_rpc(&scoped, boxed_handler).await;
            ws.emit(
                "rpc-register",
                serde_json::to_value(&RpcRegisterRequest { method: scoped }).unwrap_or_default(),
            )
            .await;
        }
    }
}
