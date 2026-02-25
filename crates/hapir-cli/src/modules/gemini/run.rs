use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error};

use hapir_shared::modes::{AgentFlavor, PermissionMode, SessionMode};
use hapir_shared::schemas::SessionStartedBy as SharedStartedBy;

use crate::agent::bootstrap::{AgentBootstrapConfig, bootstrap_agent};
use crate::agent::common_rpc::{ApplyConfigFn, CommonRpc, CommonRpcConfig, OnKillFn};
use crate::agent::loop_base::{LoopOptions, run_local_remote_session};
use crate::agent::session_base::{AgentSessionBase, AgentSessionBaseOptions};
use hapir_acp::acp_sdk::backend::AcpSdkBackend;
use hapir_acp::types::AgentBackend;
use hapir_infra::config::CliConfiguration;
use hapir_infra::rpc::{EventRegistry, RpcRegistry};
use hapir_infra::utils::message_queue::MessageQueue2;
use hapir_infra::utils::terminal::restore_terminal_state;

use super::{GeminiMode, compute_mode_hash};
use super::local_launcher::gemini_local_launcher;
use super::remote_launcher::gemini_remote_launcher;
use super::session::GeminiSession;
use crate::terminal::{TerminalManager, TerminalManagerOptions};

pub struct GeminiStartOptions {
    pub working_directory: String,
    pub runner_port: Option<u16>,
    pub started_by: SharedStartedBy,
    pub starting_mode: Option<SessionMode>,
}

pub async fn run_gemini(
    options: GeminiStartOptions,
    config: &CliConfiguration,
) -> anyhow::Result<()> {
    let started_by = options.started_by;
    let starting_mode = options.starting_mode.unwrap_or(match started_by {
        SharedStartedBy::Terminal => SessionMode::Local,
        SharedStartedBy::Runner => SessionMode::Remote,
    });

    debug!(
        "[runGemini] Starting in {} (startedBy={:?}, mode={:?})",
        options.working_directory, started_by, starting_mode
    );

    let boot = bootstrap_agent(
        AgentBootstrapConfig {
            flavor: AgentFlavor::Gemini,
            working_directory: options.working_directory.clone(),
            started_by,
            starting_mode,
            runner_port: options.runner_port,
            log_tag: "runGemini",
        },
        config,
    )
    .await?;

    let ws_client = boot.ws_client.clone();
    let working_directory = options.working_directory;

    let queue = Arc::new(MessageQueue2::new(compute_mode_hash));
    let current_mode = Arc::new(Mutex::new(GeminiMode::default()));

    let on_mode_change = boot.lifecycle.create_mode_change_handler();
    let session_base = AgentSessionBase::new(AgentSessionBaseOptions {
        ws_client: ws_client.clone(),
        path: working_directory.clone(),
        session_id: None,
        queue: queue.clone(),
        on_mode_change_cb: on_mode_change,
        mode: starting_mode,
        session_label: "gemini".to_string(),
        session_id_label: "geminiSessionId".to_string(),
        apply_session_id_to_metadata: Box::new(|mut metadata, sid| {
            metadata.gemini_session_id = Some(sid.to_string());
            metadata
        }),
        permission_mode: boot.permission_mode,
        model_mode: boot.model_mode,
    });

    let backend = Arc::new(AcpSdkBackend::new(
        "gemini".to_string(),
        vec!["--experimental-acp".to_string()],
        None,
    ));

    let apply_config: Arc<ApplyConfigFn<GeminiMode>> =
        Arc::new(Box::new(|m, params| {
            if let Some(pm) = params.get("permissionMode") {
                if let Ok(mode) = serde_json::from_value::<PermissionMode>(pm.clone()) {
                    debug!("[runGemini] Permission mode changed to: {:?}", mode);
                    m.permission_mode = Some(mode);
                }
            }
            if let Some(model) = params.get("model").and_then(|v| v.as_str()) {
                debug!("[runGemini] Model changed to: {}", model);
                m.model = Some(model.to_string());
            }
        }));

    let backend_for_kill = backend.clone();
    let on_kill: OnKillFn = Arc::new(move || {
        let b = backend_for_kill.clone();
        Box::pin(async move {
            let _ = b.disconnect().await;
        })
    });

    let rpc = CommonRpc::new(CommonRpcConfig {
        queue: queue.clone(),
        log_tag: "runGemini",
        current_mode: current_mode.clone(),
        apply_config,
        on_kill: Some(on_kill),
        switch_notify: None,
        session_mode: None,
        pre_process: None,
    });
    ws_client.register_rpc_group(&rpc).await;

    let gemini_session = GeminiSession {
        base: session_base.clone(),
        backend: backend.clone(),
    };
    ws_client.register_rpc_group(&gemini_session).await;

    let terminal_mgr = Arc::new(TerminalManager::new(TerminalManagerOptions {
        session_id: boot.session_id.clone(),
        session_path: Some(working_directory.clone()),
    }));
    ws_client.register_event_group(terminal_mgr.as_ref()).await;

    let _ = ws_client.connect(Duration::from_secs(10)).await;

    let sb_for_local = session_base.clone();
    let sb_for_remote = session_base.clone();
    let backend_for_remote = backend.clone();

    let loop_result = run_local_remote_session(LoopOptions {
        session: session_base.clone(),
        starting_mode: Some(starting_mode),
        log_tag: "runGemini".to_string(),
        run_local: Box::new(move |_base| {
            let sb = sb_for_local.clone();
            Box::pin(async move { gemini_local_launcher(&sb).await })
        }),
        run_remote: Box::new(move |_base| {
            let sb = sb_for_remote.clone();
            let b = backend_for_remote.clone();
            Box::pin(async move { gemini_remote_launcher(&sb, &b).await })
        }),
        on_session_ready: None,
        terminal_reclaim: started_by == SharedStartedBy::Terminal,
    })
    .await;

    let _ = backend.disconnect().await;
    terminal_mgr.close_all().await;
    boot.lifecycle.cleanup().await;

    restore_terminal_state();

    if let Err(e) = loop_result {
        error!("[runGemini] Loop error: {e}");
        boot.lifecycle.mark_crash(&e.to_string()).await;
    }

    Ok(())
}
