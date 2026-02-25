use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use hapir_shared::schemas::SessionStartedBy;

use super::local_launcher::claude_local_launcher;
use super::remote_launcher::claude_remote_launcher;
use crate::agent::bootstrap::{bootstrap_agent, AgentBootstrapConfig};
use crate::agent::cleanup::cleanup_agent_session;
use crate::agent::common_rpc::{
    ApplyConfigFn, CommonRpc, CommonRpcConfig, MessagePreProcessor,
};
use crate::agent::loop_base::{run_local_remote_session, LoopOptions};
use crate::agent::session_base::{AgentSessionBase, AgentSessionBaseOptions};
use crate::modules::claude::hook_server::start_hook_server;
use crate::modules::claude::session::ClaudeSession;
use crate::terminal::TerminalManager;
use crate::terminal::TerminalManagerOptions;
use hapir_infra::config::CliConfiguration;
use hapir_infra::handlers::uploads;
use hapir_infra::rpc::{EventRegistry, RpcRegistry};
use hapir_infra::utils::message_queue::MessageQueue2;
use hapir_infra::utils::terminal::save_terminal_state;
use hapir_shared::modes::{AgentFlavor, PermissionMode, SessionMode};

/// Options for starting a Claude session.
#[derive(Debug, Clone)]
pub struct ClaudeStartOptions {
    pub working_directory: String,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub starting_mode: Option<SessionMode>,
    pub should_start_runner: Option<bool>,
    pub claude_env_vars: Option<HashMap<String, String>>,
    pub claude_args: Option<Vec<String>>,
    pub started_by: SessionStartedBy,
    pub runner_port: Option<u16>,
}

/// Mode fields that affect message queue batching for Claude sessions.
#[derive(Debug, Clone, Default, Hash)]
pub struct ClaudeEnhancedMode {
    pub permission_mode: Option<PermissionMode>,
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub custom_system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
}

fn compute_mode_hash(mode: &ClaudeEnhancedMode) -> String {
    let mut hasher = DefaultHasher::new();
    mode.hash(&mut hasher);
    hasher.finish().to_string()
}

pub async fn run_claude(
    options: ClaudeStartOptions,
    config: &CliConfiguration,
) -> anyhow::Result<()> {
    let working_directory = options.working_directory.clone();

    save_terminal_state();

    let started_by = options.started_by;
    let starting_mode = options.starting_mode.unwrap_or(match started_by {
        SessionStartedBy::Terminal => SessionMode::Local,
        SessionStartedBy::Runner => SessionMode::Remote,
    });

    debug!(
        "[runClaude] Starting in {} (startedBy={:?}, mode={:?})",
        working_directory, started_by, starting_mode
    );

    let boot = bootstrap_agent(
        AgentBootstrapConfig {
            flavor: AgentFlavor::Claude,
            working_directory: working_directory.clone(),
            started_by,
            starting_mode,
            runner_port: options.runner_port,
            log_tag: "runClaude",
        },
        config,
    )
    .await?;

    let session_client = boot.ws_client.clone();
    let session_id = boot.session_id.clone();

    let initial_mode = ClaudeEnhancedMode {
        permission_mode: options.permission_mode.clone(),
        model: options.model.clone(),
        ..Default::default()
    };

    let queue = Arc::new(MessageQueue2::new(compute_mode_hash));
    let hook_session_mode = Arc::new(std::sync::Mutex::new(starting_mode));

    let base_on_mode_change = boot.lifecycle.create_mode_change_handler();
    let hook_mode_for_cb = hook_session_mode.clone();
    let on_mode_change: Box<dyn Fn(SessionMode) + Send + Sync> =
        Box::new(move |mode: SessionMode| {
            *hook_mode_for_cb.lock().unwrap() = mode;
            base_on_mode_change(mode);
        });
    let session_base = AgentSessionBase::new(AgentSessionBaseOptions {
        ws_client: session_client.clone(),
        path: working_directory.clone(),
        session_id: None,
        queue: queue.clone(),
        on_mode_change_cb: on_mode_change,
        mode: starting_mode,
        session_label: "claude".to_string(),
        session_id_label: "claudeSessionId".to_string(),
        apply_session_id_to_metadata: Box::new(|mut metadata, sid| {
            metadata.claude_session_id = Some(sid.to_string());
            metadata
        }),
        permission_mode: boot.permission_mode,
        model_mode: boot.model_mode,
    });

    let sb_for_hook = session_base.clone();
    let sb_for_thinking = session_base.clone();
    let hook_server = start_hook_server(
        Arc::new(move |sid, _data| {
            let sb = sb_for_hook.clone();
            let claude_sid = sid;
            tokio::spawn(async move {
                debug!(
                    "[runClaude] Hook server received Claude session ID: {}",
                    claude_sid
                );
                sb.on_session_found(&claude_sid).await;
            });
        }),
        Some(Arc::new(move |thinking| {
            let sb = sb_for_thinking.clone();
            tokio::spawn(async move {
                sb.on_thinking_change(thinking).await;
            });
        })),
        Some(session_client.clone()),
        hook_session_mode.clone(),
    )
    .await?;

    let hook_port = hook_server.port;
    let hook_token = hook_server.token.clone();
    debug!("[runClaude] Hook server started on port {}", hook_port);

    let hook_settings_path = write_hook_settings(&session_id, hook_port, &hook_token);

    let claude_session = Arc::new(ClaudeSession {
        base: session_base.clone(),
        claude_env_vars: options.claude_env_vars.clone(),
        claude_args: Mutex::new(options.claude_args.clone()),
        mcp_servers: HashMap::new(),
        hook_settings_path,
        started_by,
        starting_mode,
        local_launch_failure: Mutex::new(None),
        pending_permissions: Arc::new(Mutex::new(HashMap::new())),
        active_pid: Arc::new(AtomicU32::new(0)),
    });

    let current_mode = Arc::new(Mutex::new(initial_mode));

    // Prepend @path refs from attachments to the message text
    let pre_process: Arc<MessagePreProcessor> = Arc::new(Box::new(|params| {
        let text = params
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let attachment_refs: Vec<String> = params
            .get("attachments")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.get("path").and_then(|p| p.as_str()))
                    .map(|p| format!("@{p}"))
                    .collect()
            })
            .unwrap_or_default();

        if attachment_refs.is_empty() {
            text
        } else {
            let refs = attachment_refs.join(" ");
            if text.is_empty() {
                refs
            } else {
                format!("{refs}\n\n{text}")
            }
        }
    }));

    let apply_config: Arc<ApplyConfigFn<ClaudeEnhancedMode>> = Arc::new(Box::new(|m, params| {
        if let Some(pm) = params.get("permissionMode") {
            if let Ok(mode) = serde_json::from_value::<PermissionMode>(pm.clone()) {
                debug!("[runClaude] Permission mode changed to: {:?}", mode);
                m.permission_mode = Some(mode);
            }
        }
        if let Some(mm) = params.get("modelMode").and_then(|v| v.as_str()) {
            debug!("[runClaude] Model mode changed to: {}", mm);
            m.model = Some(mm.to_string());
        }
    }));

    let rpc = CommonRpc::new(CommonRpcConfig {
        queue: queue.clone(),
        log_tag: "runClaude",
        current_mode: current_mode.clone(),
        apply_config,
        on_kill: None,
        switch_notify: Some(session_base.switch_notify.clone()),
        session_mode: Some(hook_session_mode.clone()),
        pre_process: Some(pre_process),
    });
    session_client.register_rpc_group(&rpc).await;
    session_client.register_rpc_group(claude_session.as_ref()).await;

    let terminal_mgr = Arc::new(TerminalManager::new(TerminalManagerOptions {
        session_id: session_id.clone(),
        session_path: Some(working_directory.clone()),
    }));
    session_client.register_event_group(terminal_mgr.as_ref()).await;

    let _ = session_client.connect(Duration::from_secs(10)).await;

    let cs_for_local = claude_session.clone();
    let cs_for_remote = claude_session.clone();

    let loop_result = run_local_remote_session(LoopOptions {
        session: session_base.clone(),
        starting_mode: Some(starting_mode),
        log_tag: "runClaude".to_string(),
        run_local: Box::new(move |_base| {
            let cs = cs_for_local.clone();
            Box::pin(async move { claude_local_launcher(&cs).await })
        }),
        run_remote: Box::new(move |_base| {
            let cs = cs_for_remote.clone();
            Box::pin(async move { claude_remote_launcher(&cs).await })
        }),
        on_session_ready: None,
        terminal_reclaim: started_by == SessionStartedBy::Terminal,
    })
    .await;

    debug!("[runClaude] Main loop exited");

    let failure = claude_session.local_launch_failure.lock().await.take();
    if let Some(failure) = failure {
        warn!(
            "[runClaude] Local launch failed: {} ({})",
            failure.message, failure.exit_reason
        );
        session_client
            .send_message(serde_json::json!({
                "type": "error",
                "message": failure.message,
                "exitReason": failure.exit_reason,
            }))
            .await;
    }

    hook_server.stop();
    if let Err(e) = std::fs::remove_file(&claude_session.hook_settings_path) {
        debug!("[runClaude] Failed to remove hook settings file: {}", e);
    }

    uploads::cleanup_upload_dir(&session_id).await;

    cleanup_agent_session(loop_result, terminal_mgr, boot.lifecycle, true, "runClaude").await;

    Ok(())
}

fn write_hook_settings(session_id: &str, hook_port: u16, hook_token: &str) -> String {
    let hook_settings_path = format!(
        "{}/.hapir/hook-settings/{}.json",
        dirs_next::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        session_id
    );
    let mut exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "hapir".to_string());
    if cfg!(windows) {
        exe_path = exe_path.replace('\\', "/");
    }
    let session_start_cmd = format!(
        "{} hook-forwarder --port {} --token {} --endpoint hook/session-start",
        exe_path, hook_port, hook_token
    );
    let event_cmd = format!(
        "{} hook-forwarder --port {} --token {} --endpoint hook/event",
        exe_path, hook_port, hook_token
    );
    let settings_json = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": session_start_cmd
                        }
                    ]
                }
            ],
            "UserPromptSubmit": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": &event_cmd
                        }
                    ]
                }
            ],
            "Stop": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": &event_cmd
                        }
                    ]
                }
            ]
        }
    });
    if let Some(parent) = std::path::Path::new(&hook_settings_path).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    match std::fs::write(&hook_settings_path, settings_json.to_string()) {
        Ok(_) => debug!("[runClaude] Wrote hook settings to {}", hook_settings_path),
        Err(e) => warn!("[runClaude] Failed to write hook settings: {}", e),
    }
    hook_settings_path
}
