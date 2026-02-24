use std::sync::Arc;

use tracing::{debug, error};
use hapir_infra::utils::terminal::restore_terminal_state;
use crate::agent::session_lifecycle::AgentSessionLifecycle;
use crate::terminal::TerminalManager;

/// Common cleanup sequence shared by all agent `run_*` functions.
///
/// Closes all terminals, runs lifecycle cleanup, optionally restores the
/// terminal state, and marks a crash if the loop returned an error.
pub async fn cleanup_agent_session(
    loop_result: anyhow::Result<()>,
    terminal_mgr: Arc<TerminalManager>,
    lifecycle: Arc<AgentSessionLifecycle>,
    restore_terminal: bool,
    log_tag: &str,
) {
    debug!("[{log_tag}] Main loop exited");

    terminal_mgr.close_all().await;
    lifecycle.cleanup().await;

    if restore_terminal {
        restore_terminal_state();
    }

    if let Err(e) = loop_result {
        error!("[{log_tag}] Loop error: {e}");
        lifecycle.mark_crash(&e.to_string()).await;
    }
}
