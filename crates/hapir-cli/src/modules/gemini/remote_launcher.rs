use std::sync::Arc;

use crate::agent::acp_launcher::acp_remote_launcher;
use crate::agent::loop_base::LoopResult;
use crate::agent::session_base::AgentSessionBase;
use hapir_acp::acp_sdk::backend::AcpSdkBackend;

use super::GeminiMode;

pub async fn gemini_remote_launcher(
    session: &Arc<AgentSessionBase<GeminiMode>>,
    backend: &Arc<AcpSdkBackend>,
) -> LoopResult {
    acp_remote_launcher(session, backend, "geminiRemoteLauncher", "gemini").await
}
