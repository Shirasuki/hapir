use std::sync::Arc;

use tracing::{debug, warn};

use hapir_shared::modes::{AgentFlavor, ModelMode, PermissionMode, SessionMode};
use hapir_shared::schemas::SessionStartedBy;

use crate::agent::session_lifecycle::{AgentSessionLifecycle, AgentSessionLifecycleOptions};
use crate::agent::session_init::{bootstrap_session, SessionBootstrapOptions};
use hapir_infra::api::ApiClient;
use hapir_infra::config::CliConfiguration;
use hapir_infra::ws::session_client::WsSessionClient;
use hapir_runner::control_client;
use hapir_runner::types::SessionStartedMetadata;

/// Configuration for bootstrapping an agent session.
pub struct AgentBootstrapConfig {
    pub flavor: AgentFlavor,
    pub working_directory: String,
    pub started_by: SessionStartedBy,
    pub starting_mode: SessionMode,
    pub runner_port: Option<u16>,
    pub log_tag: &'static str,
}

/// Result of bootstrapping an agent session.
pub struct AgentBootstrapResult {
    pub ws_client: Arc<WsSessionClient>,
    pub api: Arc<ApiClient>,
    pub session_id: String,
    pub lifecycle: Arc<AgentSessionLifecycle>,
    pub permission_mode: Option<PermissionMode>,
    pub model_mode: Option<ModelMode>,
}

/// Shared bootstrap pipeline for all agent `run_*` functions.
///
/// Bootstraps session + WS client, notifies the runner,
/// registers signal handlers, and sets the initial `controlledByUser` state.
pub async fn bootstrap_agent(
    cfg: AgentBootstrapConfig,
    config: &CliConfiguration,
) -> anyhow::Result<AgentBootstrapResult> {
    let bootstrap = bootstrap_session(
        SessionBootstrapOptions {
            flavor: cfg.flavor,
            started_by: Some(cfg.started_by),
            working_directory: Some(cfg.working_directory),
            tag: None,
            agent_state: Some(serde_json::json!({
                "controlledByUser": cfg.starting_mode == SessionMode::Local,
            })),
        },
        &config,
    )
    .await?;

    let ws_client = bootstrap.ws_client.clone();
    let api = bootstrap.api.clone();
    let session_id = bootstrap.session_info.id.clone();

    debug!("[{}] Session bootstrapped: {}", cfg.log_tag, session_id);

    if let Some(port) = cfg.runner_port {
        let pid = std::process::id();
        if let Err(e) = control_client::notify_session_started(
            port,
            &session_id,
            Some(SessionStartedMetadata { host_pid: pid }),
        )
        .await
        {
            warn!(
                "[{}] Failed to notify runner of session start: {e}",
                cfg.log_tag
            );
        }
    }

    let lifecycle = AgentSessionLifecycle::new(AgentSessionLifecycleOptions {
        ws_client: ws_client.clone(),
        log_tag: cfg.log_tag.to_string(),
        stop_keep_alive: None,
        on_before_close: None,
        on_after_close: None,
    });
    lifecycle.register_process_handlers();

    lifecycle.set_controlled_by_user(cfg.starting_mode).await;

    Ok(AgentBootstrapResult {
        ws_client,
        api,
        session_id,
        lifecycle,
        permission_mode: bootstrap.session_info.permission_mode,
        model_mode: bootstrap.session_info.model_mode,
    })
}
