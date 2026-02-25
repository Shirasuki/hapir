use std::sync::Arc;

use tracing::{debug, warn};

use hapir_shared::schemas::SessionStartedBy as SharedStartedBy;
use hapir_shared::session::FlatMessage;

use crate::agent::local_launch_policy::{
    LocalLaunchContext, LocalLaunchExitReason, get_local_launch_exit_reason,
};
use crate::agent::loop_base::LoopResult;
use crate::agent::session_base::AgentSessionBase;

use super::GeminiMode;

pub async fn gemini_local_launcher(session: &Arc<AgentSessionBase<GeminiMode>>) -> LoopResult {
    let working_directory = session.path.clone();
    debug!("[geminiLocalLauncher] Starting in {}", working_directory);

    let mut cmd = tokio::process::Command::new("gemini");
    cmd.current_dir(&working_directory);

    match cmd.status().await {
        Ok(status) => {
            debug!("[geminiLocalLauncher] Gemini process exited: {:?}", status);
        }
        Err(e) => {
            warn!("[geminiLocalLauncher] Failed to spawn gemini: {}", e);
            session
                .ws_client
                .send_typed_message(&FlatMessage::Error {
                    message: format!("Failed to launch gemini CLI: {}", e),
                    exit_reason: None,
                })
                .await;
        }
    }

    let exit_reason = get_local_launch_exit_reason(&LocalLaunchContext {
        started_by: Some(SharedStartedBy::Terminal),
        starting_mode: Some(*session.mode.lock().await),
    });

    debug!("[geminiLocalLauncher] Exit reason: {:?}", exit_reason);

    match exit_reason {
        LocalLaunchExitReason::Switch => LoopResult::Switch,
        LocalLaunchExitReason::Exit => LoopResult::Exit,
    }
}
