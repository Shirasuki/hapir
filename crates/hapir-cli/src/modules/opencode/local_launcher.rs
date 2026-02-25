use std::sync::Arc;

use tracing::{debug, warn};

use hapir_shared::modes::SessionMode;
use hapir_shared::session::FlatMessage;

use crate::agent::local_launch_policy::{
    LocalLaunchContext, LocalLaunchExitReason, get_local_launch_exit_reason,
};
use crate::agent::loop_base::LoopResult;
use crate::agent::session_base::AgentSessionBase;

use super::OpencodeMode;

pub async fn opencode_local_launcher(session: &Arc<AgentSessionBase<OpencodeMode>>) -> LoopResult {
    let working_directory = session.path.clone();
    debug!("[opencodeLocalLauncher] Starting in {}", working_directory);

    let mut cmd = tokio::process::Command::new("opencode");
    cmd.current_dir(&working_directory);

    let _exit_status = match cmd.status().await {
        Ok(status) => {
            debug!("[opencodeLocalLauncher] opencode exited: {:?}", status);
            Some(status)
        }
        Err(e) => {
            warn!("[opencodeLocalLauncher] Failed to spawn opencode: {}", e);
            session
                .ws_client
                .send_typed_message(&FlatMessage::Error {
                    message: format!("Failed to launch opencode CLI: {}", e),
                    exit_reason: None,
                })
                .await;
            None
        }
    };

    let exit_reason = get_local_launch_exit_reason(&LocalLaunchContext {
        started_by: None,
        starting_mode: Some(SessionMode::Local),
    });

    match exit_reason {
        LocalLaunchExitReason::Switch => LoopResult::Switch,
        LocalLaunchExitReason::Exit => LoopResult::Exit,
    }
}
