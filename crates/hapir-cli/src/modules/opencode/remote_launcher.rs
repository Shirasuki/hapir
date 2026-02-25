use std::sync::Arc;

use tracing::{debug, warn};

use hapir_shared::session::FlatMessage;

use crate::agent::agent_message_convert::agent_message_to_flat;
use crate::agent::loop_base::LoopResult;
use crate::agent::session_base::AgentSessionBase;
use hapir_acp::acp_sdk::backend::AcpSdkBackend;
use hapir_acp::types::{AgentBackend, AgentMessage, AgentSessionConfig, PromptContent};
use hapir_infra::ws::session_client::WsSessionClient;

use super::OpencodeMode;

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

pub async fn opencode_remote_launcher(
    session: &Arc<AgentSessionBase<OpencodeMode>>,
    backend: &Arc<AcpSdkBackend>,
) -> LoopResult {
    let working_directory = session.path.clone();
    debug!("[opencodeRemoteLauncher] Starting in {}", working_directory);

    if let Err(e) = backend.initialize().await {
        warn!(
            "[opencodeRemoteLauncher] Failed to initialize ACP backend: {}",
            e
        );
        session
            .ws_client
            .send_typed_message(&FlatMessage::Error {
                message: format!("Failed to initialize opencode ACP: {}", e),
                exit_reason: None,
            })
            .await;
        return LoopResult::Exit;
    }

    let acp_session_id = match backend
        .new_session(AgentSessionConfig {
            cwd: working_directory.clone(),
            mcp_servers: vec![],
        })
        .await
    {
        Ok(sid) => {
            session.on_session_found(&sid).await;
            sid
        }
        Err(e) => {
            warn!(
                "[opencodeRemoteLauncher] Failed to create ACP session: {}",
                e
            );
            session
                .ws_client
                .send_typed_message(&FlatMessage::Error {
                    message: format!("Failed to create opencode ACP session: {}", e),
                    exit_reason: None,
                })
                .await;
            return LoopResult::Exit;
        }
    };

    loop {
        let batch = match session.queue.wait_for_messages().await {
            Some(batch) => batch,
            None => {
                debug!("[opencodeRemoteLauncher] Queue closed, exiting");
                return LoopResult::Exit;
            }
        };

        let prompt = batch.message;
        debug!(
            "[opencodeRemoteLauncher] Processing message: {}",
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
            warn!("[opencodeRemoteLauncher] Prompt error: {}", e);
            session
                .ws_client
                .send_typed_message(&FlatMessage::Error {
                    message: format!("OpenCode ACP error: {}", e),
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
