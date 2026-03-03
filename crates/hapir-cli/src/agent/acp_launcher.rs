use std::sync::Arc;

use tracing::{debug, warn};

use hapir_acp::acp_sdk::backend::AcpSdkBackend;
use hapir_acp::types::{AgentBackend, AgentMessage, AgentSessionConfig, PromptContent};
use hapir_infra::ws::session_client::WsSessionClient;
use hapir_shared::common::session_messages::FlatMessage;

use super::agent_message_convert::agent_message_to_flat;
use super::loop_base::LoopResult;
use super::session_base::{AgentSessionBase, STATUS_STARTING};

async fn forward_agent_message(ws: &WsSessionClient, msg: AgentMessage) {
    if let AgentMessage::TextDelta {
        message_id,
        text,
        is_final,
    } = &msg
    {
        ws.send_message_delta(message_id, text, *is_final).await;
        return;
    }
    if let Some(flat) = agent_message_to_flat(&msg) {
        ws.send_typed_message(&flat).await;
    }
}

/// Generic remote launcher for ACP-based agent backends (Gemini, OpenCode).
///
/// Handles the full lifecycle: broadcast startup status, initialize backend,
/// create session, run message loop, and clean up thinking state on exit.
pub async fn acp_remote_launcher<Mode: Clone + Send + 'static>(
    session: &Arc<AgentSessionBase<Mode>>,
    backend: &Arc<AcpSdkBackend>,
    log_tag: &str,
    agent_name: &str,
) -> LoopResult {
    let working_directory = session.path.clone();
    debug!("[{}] Starting in {}", log_tag, working_directory);

    session.start_thinking_with_status(STATUS_STARTING).await;

    if let Err(e) = backend.initialize().await {
        warn!("[{}] Failed to initialize ACP backend: {}", log_tag, e);
        session.on_thinking_change(false).await;
        session
            .ws_client
            .send_typed_message(&FlatMessage::Error {
                message: format!("Failed to initialize {} ACP: {}", agent_name, e),
                exit_reason: None,
            })
            .await;
        return LoopResult::Exit;
    }

    let acp_session_id = match backend
        .new_session(AgentSessionConfig {
            cwd: working_directory,
            mcp_servers: vec![],
        })
        .await
    {
        Ok(sid) => {
            session.on_session_found(&sid).await;
            sid
        }
        Err(e) => {
            warn!("[{}] Failed to create ACP session: {}", log_tag, e);
            session.on_thinking_change(false).await;
            session
                .ws_client
                .send_typed_message(&FlatMessage::Error {
                    message: format!("Failed to create {} ACP session: {}", agent_name, e),
                    exit_reason: None,
                })
                .await;
            return LoopResult::Exit;
        }
    };

    session.on_thinking_change(false).await;

    loop {
        let batch = match session.queue.wait_for_messages().await {
            Some(batch) => batch,
            None => {
                debug!("[{}] Queue closed, exiting", log_tag);
                return LoopResult::Exit;
            }
        };

        let prompt = batch.message;
        debug!(
            "[{}] Processing message: {}",
            log_tag,
            if prompt.len() > 100 {
                format!("{}...", &prompt[..prompt.floor_char_boundary(100)])
            } else {
                prompt.clone()
            }
        );

        session.on_thinking_change(true).await;

        let ws_for_update = session.ws_client.clone();
        let on_update: Box<dyn Fn(AgentMessage) + Send + Sync> = Box::new(move |msg| {
            let ws = ws_for_update.clone();
            tokio::spawn(async move {
                forward_agent_message(&ws, msg).await;
            });
        });

        let content = vec![PromptContent::Text { text: prompt }];
        if let Err(e) = backend.prompt(&acp_session_id, content, on_update).await {
            warn!("[{}] Prompt error: {}", log_tag, e);
            session
                .ws_client
                .send_typed_message(&FlatMessage::Error {
                    message: format!("{} ACP error: {}", agent_name, e),
                    exit_reason: None,
                })
                .await;
        }

        session.on_thinking_change(false).await;

        if session.queue.is_closed().await {
            return LoopResult::Exit;
        }
    }
}
