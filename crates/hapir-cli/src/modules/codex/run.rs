use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{debug, error};

use hapir_shared::modes::{AgentFlavor, PermissionMode, SessionMode};
use hapir_shared::schemas::SessionStartedBy as SharedStartedBy;

use crate::agent::bootstrap::{AgentBootstrapConfig, bootstrap_agent};
use crate::agent::common_rpc::{
    ApplyConfigFn, CommonRpc, CommonRpcConfig, MessagePreProcessor, OnKillFn,
};
use crate::agent::loop_base::{LoopOptions, run_local_remote_session};
use crate::agent::session_base::{AgentSessionBase, AgentSessionBaseOptions};
use hapir_acp::codex_app_server::backend::CodexAppServerBackend;
use hapir_acp::types::AgentBackend;
use hapir_infra::config::CliConfiguration;
use hapir_infra::rpc::{EventRegistry, RpcRegistry};
use hapir_infra::utils::message_queue::MessageQueue2;
use hapir_infra::utils::terminal::restore_terminal_state;

use super::local_launcher::codex_local_launcher;
use super::remote_launcher::codex_remote_launcher;
use super::session::CodexSession;
use super::{CodexMode, compute_mode_hash};
use crate::terminal::{TerminalManager, TerminalManagerOptions};

pub struct CodexStartOptions {
    pub working_directory: String,
    pub runner_port: Option<u16>,
    pub started_by: SharedStartedBy,
    pub starting_mode: Option<SessionMode>,
    pub model: Option<String>,
    pub yolo: bool,
    pub resume: Option<String>,
}

pub async fn run_codex(
    options: CodexStartOptions,
    config: &CliConfiguration,
) -> anyhow::Result<()> {
    let working_directory = options.working_directory;

    let started_by = options.started_by;
    let starting_mode = options.starting_mode.unwrap_or(match started_by {
        SharedStartedBy::Terminal => SessionMode::Local,
        SharedStartedBy::Runner => SessionMode::Remote,
    });

    debug!(
        "[runCodex] Starting in {} (startedBy={:?}, mode={:?})",
        working_directory, started_by, starting_mode
    );

    let boot = bootstrap_agent(
        AgentBootstrapConfig {
            flavor: AgentFlavor::Codex,
            working_directory: working_directory.clone(),
            started_by,
            starting_mode,
            runner_port: options.runner_port,
            log_tag: "runCodex",
        },
        config,
    )
    .await?;

    let ws_client = boot.ws_client.clone();

    let initial_mode = CodexMode {
        model: options.model,
        ..Default::default()
    };
    let queue = Arc::new(MessageQueue2::new(compute_mode_hash));
    let current_mode = Arc::new(Mutex::new(initial_mode));

    let on_mode_change = boot.lifecycle.create_mode_change_handler();
    let session_base = AgentSessionBase::new(AgentSessionBaseOptions {
        ws_client: ws_client.clone(),
        path: working_directory.clone(),
        session_id: None,
        queue: queue.clone(),
        on_mode_change_cb: on_mode_change,
        mode: starting_mode,
        session_label: "codex".to_string(),
        session_id_label: "codexSessionId".to_string(),
        apply_session_id_to_metadata: Box::new(|mut metadata, sid| {
            metadata.codex_session_id = Some(sid.to_string());
            metadata
        }),
        permission_mode: boot.permission_mode,
        model_mode: boot.model_mode,
    });

    let mut codex_args = vec!["app-server".to_string()];
    if options.yolo {
        codex_args.push("--full-auto".to_string());
    }
    let backend = Arc::new(CodexAppServerBackend::new(
        "codex".to_string(),
        codex_args,
        None,
    ));

    let pending_attachments: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let attachments_for_rpc = pending_attachments.clone();

    // Extract attachment paths from params and accumulate them for the next prompt
    let pre_process: Arc<MessagePreProcessor> = Arc::new(Box::new(move |params| {
        let text = params
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(attachments) = params.get("attachments").and_then(|v| v.as_array()) {
            let paths: Vec<String> = attachments
                .iter()
                .filter_map(|a| a.get("path").and_then(|p| p.as_str()))
                .map(|s| s.to_string())
                .collect();
            if !paths.is_empty() {
                debug!("[runCodex] Received {} attachment(s)", paths.len());
                let att = attachments_for_rpc.clone();
                tokio::spawn(async move {
                    att.lock().await.extend(paths);
                });
            }
        }

        text
    }));

    let apply_config: Arc<ApplyConfigFn<CodexMode>> =
        Arc::new(Box::new(|m, params| {
            if let Some(pm) = params.get("permissionMode") {
                if let Ok(mode) = serde_json::from_value::<PermissionMode>(pm.clone()) {
                    debug!("[runCodex] Permission mode changed to: {:?}", mode);
                    m.permission_mode = Some(mode);
                }
            }
            if let Some(cm) = params.get("collaborationMode").and_then(|v| v.as_str()) {
                debug!("[runCodex] Collaboration mode changed to: {}", cm);
                m.collaboration_mode = Some(cm.to_string());
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
        log_tag: "runCodex",
        current_mode: current_mode.clone(),
        apply_config,
        on_kill: Some(on_kill),
        switch_notify: Some(session_base.switch_notify.clone()),
        session_mode: Some(Arc::new(std::sync::Mutex::new(starting_mode))),
        pre_process: Some(pre_process),
    });
    ws_client.register_rpc_group(&rpc).await;

    let codex_session = CodexSession {
        base: session_base.clone(),
        backend: backend.clone(),
    };
    ws_client.register_rpc_group(&codex_session).await;
    codex_session.setup_permission_forwarding();

    let terminal_mgr = Arc::new(TerminalManager::new(TerminalManagerOptions {
        session_id: boot.session_id.clone(),
        session_path: Some(working_directory.clone()),
    }));
    ws_client.register_event_group(terminal_mgr.as_ref()).await;

    let _ = ws_client.connect(Duration::from_secs(10)).await;

    let sb_for_local = session_base.clone();
    let sb_for_remote = session_base.clone();
    let backend_for_remote = backend.clone();
    let resume_thread_id: Option<String> = options.resume;
    let attachments_for_remote = pending_attachments.clone();

    let loop_result = run_local_remote_session(LoopOptions {
        session: session_base.clone(),
        starting_mode: Some(starting_mode),
        log_tag: "runCodex".to_string(),
        run_local: Box::new(move |_base| {
            let sb = sb_for_local.clone();
            Box::pin(async move { codex_local_launcher(&sb).await })
        }),
        run_remote: Box::new(move |_base| {
            let sb = sb_for_remote.clone();
            let b = backend_for_remote.clone();
            let resume = resume_thread_id.clone();
            let att = attachments_for_remote.clone();
            Box::pin(async move { codex_remote_launcher(&sb, &b, resume.as_deref(), att).await })
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
        error!("[runCodex] Loop error: {e}");
        boot.lifecycle.mark_crash(&e.to_string()).await;
    }

    Ok(())
}
