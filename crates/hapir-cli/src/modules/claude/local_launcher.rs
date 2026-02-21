use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::time::{self, Duration};
use tracing::{debug, info, warn};

use crate::agent::loop_base::LoopResult;
use crate::modules::claude::session::ClaudeSession;
use crate::modules::claude::session_scanner::SessionScanner;
use crate::modules::claude::types::RawJsonLines;

use super::run::EnhancedMode;

/// Local launcher for Claude.
///
/// Spawns the `claude` CLI process in local/interactive mode,
/// sets up a session scanner to watch for session IDs, and manages
/// the local launch lifecycle.
///
/// If a web message arrives in the queue while the local process is running,
/// the local process is terminated and we return `Switch` so the remote
/// launcher can handle it.
pub async fn claude_local_launcher(session: &Arc<ClaudeSession<EnhancedMode>>) -> LoopResult {
    let working_directory = session.base.path.clone();
    debug!("[claudeLocalLauncher] Starting in {}", working_directory);

    // Create session scanner that forwards local session messages to the web UI.
    let ws_for_scanner = session.base.ws_client.clone();
    let scanner = Arc::new(Mutex::new(SessionScanner::new(
        session.base.session_id.lock().await.clone(),
        &working_directory,
        Box::new(move |message: RawJsonLines| {
            let ws = ws_for_scanner.clone();
            tokio::spawn(async move {
                forward_session_message(&ws, message).await;
            });
        }),
    )));

    // Seed the scanner with existing messages so they aren't re-emitted.
    scanner.lock().await.seed().await;

    // Register session-found callback on the base session
    let scanner_for_cb = scanner.clone();
    session
        .base
        .add_session_found_callback(Box::new(move |sid| {
            let scanner = scanner_for_cb.clone();
            let sid = sid.to_string();
            tokio::spawn(async move {
                scanner.lock().await.on_new_session(&sid);
            });
        }))
        .await;

    // Start a polling task that scans session files every 3 seconds.
    let scanner_for_poll = scanner.clone();
    let poll_handle = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(3));
        loop {
            interval.tick().await;
            scanner_for_poll.lock().await.scan().await;
        }
    });

    // Build CLI arguments for local (interactive) mode
    let mut args: Vec<String> = Vec::new();

    // Add hook settings if available
    if !session.hook_settings_path.is_empty() {
        args.push("--settings".to_string());
        args.push(session.hook_settings_path.clone());
    }

    // Resume the previous conversation if we have a session ID
    // (e.g. switching back from remote mode)
    if let Some(ref sid) = *session.base.session_id.lock().await {
        args.push("--resume".to_string());
        args.push(sid.clone());
    }

    // Add any passthrough claude args
    if let Some(ref extra_args) = *session.claude_args.lock().await {
        args.extend(extra_args.iter().cloned());
    }

    // Spawn the claude CLI process in interactive mode
    debug!(
        "[claudeLocalLauncher] Spawning claude process with args: {:?}",
        args
    );

    let mut cmd = tokio::process::Command::new("claude");
    cmd.args(&args).current_dir(&working_directory);

    // Pass through claude env vars
    if let Some(ref env_vars) = session.claude_env_vars {
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
    }

    let mut switch_to_remote = false;

    match cmd.spawn() {
        Ok(mut child) => {
            // Wait for either: child exits, or a web message arrives in the queue
            let queue = &session.base.queue;
            loop {
                tokio::select! {
                    status = child.wait() => {
                        match status {
                            Ok(s) => {
                                debug!("[claudeLocalLauncher] Claude process exited: {:?}", s);
                                session.consume_one_time_flags().await;
                            }
                            Err(e) => {
                                warn!("[claudeLocalLauncher] Error waiting for claude: {}", e);
                            }
                        }
                        break;
                    }
                    _ = queue.notified() => {
                        if queue.has_messages().await {
                            info!("[claudeLocalLauncher] Web message received, switching to remote mode");
                            // Kill the local process so the terminal is freed
                            let _ = child.kill().await;
                            switch_to_remote = true;
                            break;
                        }
                    }
                }
            }
        }
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
        }
    };

    // Stop polling and do a final scan to capture any remaining messages
    poll_handle.abort();
    scanner.lock().await.scan().await;

    // Cleanup scanner
    scanner.lock().await.cleanup().await;

    if switch_to_remote {
        return LoopResult::Switch;
    }

    // After local claude exits, switch to remote so pending web messages
    // can be processed by the remote launcher.
    LoopResult::Switch
}

/// Forward a raw JSONL session message to the web UI.
///
/// Mirrors the reference implementation's `sendClaudeSessionMessage`:
/// - User messages with string content → `{ role: "user", content: { type: "text", text } }`
/// - Everything else → `{ role: "assistant", content: { type: "output", data: body } }`
async fn forward_session_message(
    ws: &hapir_infra::ws::session_client::WsSessionClient,
    message: RawJsonLines,
) {
    // Skip summary messages — the remote launcher generates its own
    if matches!(&message, RawJsonLines::Summary { .. }) {
        return;
    }

    let body = match &message {
        RawJsonLines::User {
            message: raw_msg, ..
        } => {
            // If user message content is a plain string, send as text
            if let Some(content) = &raw_msg.content
                && let Some(text) = content.as_str()
            {
                serde_json::json!({
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": text,
                    },
                    "meta": { "sentFrom": "cli" }
                })
            } else {
                serde_json::json!({
                    "role": "assistant",
                    "content": {
                        "type": "output",
                        "data": message,
                    },
                    "meta": { "sentFrom": "cli" }
                })
            }
        }
        _ => {
            serde_json::json!({
                "role": "assistant",
                "content": {
                    "type": "output",
                    "data": message,
                },
                "meta": { "sentFrom": "cli" }
            })
        }
    };

    ws.send_message(body).await;
}
