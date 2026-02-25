use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error};

use hapir_shared::modes::{AgentFlavor, PermissionMode, SessionMode};
use hapir_shared::schemas::SessionStartedBy;

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

use super::{OpencodeMode, compute_mode_hash};
use super::local_launcher::opencode_local_launcher;
use super::remote_launcher::opencode_remote_launcher;
use super::session::OpencodeSession;
use crate::terminal::{TerminalManager, TerminalManagerOptions};

pub struct OpencodeStartOptions {
    pub working_directory: String,
    pub runner_port: Option<u16>,
    pub started_by: SessionStartedBy,
    pub starting_mode: Option<SessionMode>,
    pub resume: Option<String>,
}

pub async fn run_opencode(
    options: OpencodeStartOptions,
    config: &CliConfiguration,
) -> anyhow::Result<()> {
    let started_by = options.started_by;
    let starting_mode = options.starting_mode.unwrap_or(match started_by {
        SessionStartedBy::Terminal => SessionMode::Local,
        SessionStartedBy::Runner => SessionMode::Remote,
    });

    debug!(
        "[runOpenCode] Starting in {} (startedBy={:?}, mode={:?})",
        options.working_directory, started_by, starting_mode
    );

    let boot = bootstrap_agent(
        AgentBootstrapConfig {
            flavor: AgentFlavor::Opencode,
            working_directory: options.working_directory.clone(),
            started_by,
            starting_mode,
            runner_port: options.runner_port,
            log_tag: "runOpenCode",
        },
        config,
    )
    .await?;

    let ws_client = boot.ws_client.clone();
    let working_directory = options.working_directory;

    let queue = Arc::new(MessageQueue2::new(compute_mode_hash));
    let current_mode = Arc::new(Mutex::new(OpencodeMode::default()));

    let on_mode_change = boot.lifecycle.create_mode_change_handler();
    let session_base = AgentSessionBase::new(AgentSessionBaseOptions {
        ws_client: ws_client.clone(),
        path: working_directory.clone(),
        session_id: None,
        queue: queue.clone(),
        on_mode_change_cb: on_mode_change,
        mode: starting_mode,
        session_label: "opencode".to_string(),
        session_id_label: "opencodeSessionId".to_string(),
        apply_session_id_to_metadata: Box::new(|mut metadata, sid| {
            metadata.opencode_session_id = Some(sid.to_string());
            metadata
        }),
        permission_mode: boot.permission_mode,
        model_mode: boot.model_mode,
    });

    let backend = Arc::new(AcpSdkBackend::new(
        "opencode".to_string(),
        vec![
            "acp".to_string(),
            "--cwd".to_string(),
            working_directory.to_string(),
        ],
        None,
    ));

    let apply_config: Arc<ApplyConfigFn<OpencodeMode>> =
        Arc::new(Box::new(|m, params| {
            if let Some(pm) = params.get("permissionMode") {
                if let Ok(mode) = serde_json::from_value::<PermissionMode>(pm.clone()) {
                    debug!("[runOpenCode] Permission mode changed to: {:?}", mode);
                    m.permission_mode = Some(mode);
                }
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
        log_tag: "runOpenCode",
        current_mode: current_mode.clone(),
        apply_config,
        on_kill: Some(on_kill),
        switch_notify: None,
        session_mode: None,
        pre_process: None,
    });
    ws_client.register_rpc_group(&rpc).await;

    let opencode_session = OpencodeSession {
        base: session_base.clone(),
        backend: backend.clone(),
    };
    ws_client.register_rpc_group(&opencode_session).await;

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
        log_tag: "runOpenCode".to_string(),
        run_local: Box::new(move |_base| {
            let s = sb_for_local.clone();
            Box::pin(async move { opencode_local_launcher(&s).await })
        }),
        run_remote: Box::new(move |_base| {
            let s = sb_for_remote.clone();
            let b = backend_for_remote.clone();
            Box::pin(async move { opencode_remote_launcher(&s, &b).await })
        }),
        on_session_ready: None,
        terminal_reclaim: started_by == SessionStartedBy::Terminal,
    })
    .await;

    let _ = backend.disconnect().await;
    terminal_mgr.close_all().await;
    boot.lifecycle.cleanup().await;

    restore_terminal_state();

    if let Err(e) = loop_result {
        error!("[runOpenCode] Loop error: {e}");
        boot.lifecycle.mark_crash(&e.to_string()).await;
    }

    Ok(())
}
