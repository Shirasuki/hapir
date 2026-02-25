use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::debug;

use crate::agent::session_base::AgentSessionBase;
use hapir_acp::acp_sdk::backend::AcpSdkBackend;
use hapir_acp::types::AgentBackend;
use hapir_infra::rpc::{RpcHandlerGroup, RpcRegistry};
use hapir_infra::ws::session_client::WsSessionClient;

use super::OpencodeMode;

pub struct OpencodeSession {
    pub base: Arc<AgentSessionBase<OpencodeMode>>,
    pub backend: Arc<AcpSdkBackend>,
}

impl RpcHandlerGroup<WsSessionClient> for OpencodeSession {
    fn register_rpc_handlers<'a>(
        &'a self,
        ws: &'a WsSessionClient,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let backend = self.backend.clone();
            let base = self.base.clone();
            ws.register_rpc("abort", move |_params| {
                let b = backend.clone();
                let sb = base.clone();
                async move {
                    debug!("[runOpenCode] abort RPC received");
                    if let Some(sid) = sb.session_id.lock().await.clone() {
                        let _ = b.cancel_prompt(&sid).await;
                    }
                    sb.on_thinking_change(false).await;
                    serde_json::json!({"ok": true})
                }
            })
            .await;
        })
    }
}
