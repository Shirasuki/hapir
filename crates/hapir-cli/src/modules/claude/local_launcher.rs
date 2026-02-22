use crate::agent::local_launch_policy::{
    LocalLaunchContext, LocalLaunchExitReason, get_local_launch_exit_reason,
};
use crate::agent::loop_base::LoopResult;
use crate::modules::claude::session::ClaudeSession;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::process::Command;
use tokio::select;
use tracing::{debug, info, warn};

use super::run::EnhancedMode;

/// Local launcher for Claude.
///
/// Spawns the `claude` CLI process in local/interactive mode and manages
/// the local launch lifecycle. Message sync is handled by the hook server.
///
/// Listens for `switch_notify` to allow external mode-switch requests
/// (e.g. from the web UI) to kill the TUI and trigger a switch to remote.
pub async fn claude_local_launcher(session: &Arc<ClaudeSession<EnhancedMode>>) -> LoopResult {
    let working_directory = session.base.path.clone();
    debug!("[claudeLocalLauncher] Starting in {}", working_directory);

    let mut args: Vec<String> = Vec::new();

    if !session.hook_settings_path.is_empty() {
        args.push("--settings".to_string());
        args.push(session.hook_settings_path.clone());
    }

    // Resume the existing Claude session if one was established in remote mode
    if let Some(ref sid) = *session.base.session_id.lock().await {
        args.push("--resume".to_string());
        args.push(sid.clone());
    }

    if let Some(ref extra_args) = *session.claude_args.lock().await {
        args.extend(extra_args.iter().cloned());
    }

    debug!(
        "[claudeLocalLauncher] Spawning claude process with args: {:?}",
        args
    );

    let mut cmd = Command::new("claude");
    cmd.args(&args).current_dir(&working_directory);

    if let Some(ref env_vars) = session.claude_env_vars {
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            warn!("[claudeLocalLauncher] Failed to spawn claude: {}", e);
            session.record_local_launch_failure(
                format!("Failed to launch claude: {}", e),
                "spawn_error".to_string(),
            );
            session
                .base
                .ws_client
                .send_message(serde_json::json!({
                    "type": "error",
                    "message": format!("Failed to launch claude CLI: {}", e),
                }))
                .await;
            return LoopResult::Exit;
        }
    };

    if let Some(pid) = child.id() {
        session.active_pid.store(pid, Ordering::Relaxed);
    }

    let switched = select! {
        status = child.wait() => {
            match status {
                Ok(s) => debug!("[claudeLocalLauncher] Claude process exited: {:?}", s),
                Err(e) => warn!("[claudeLocalLauncher] Error waiting for claude: {}", e),
            }
            session.consume_one_time_flags().await;
            false
        }
        _ = session.base.switch_notify.notified() => {
            info!("[claudeLocalLauncher] Switch requested, killing TUI process");
            let _ = child.kill().await;
            true
        }
    };

    session.active_pid.store(0, Ordering::Relaxed);

    if switched {
        return LoopResult::Switch;
    }

    let exit_reason = get_local_launch_exit_reason(&LocalLaunchContext {
        started_by: Some(session.started_by),
        starting_mode: Some(session.starting_mode),
    });

    debug!("[claudeLocalLauncher] Exit reason: {:?}", exit_reason);

    match exit_reason {
        LocalLaunchExitReason::Switch => LoopResult::Switch,
        LocalLaunchExitReason::Exit => LoopResult::Exit,
    }
}
