use std::sync::Arc;

use serde_json::json;
use tracing::{info, warn};

use crate::agent::session_factory::build_machine_metadata;
use crate::commands::common;
use crate::config::Configuration;
use crate::persistence;
use crate::runner::{control_client, control_server};
use crate::ws::machine_client::WsMachineClient;

pub async fn run(action: Option<&str>) -> anyhow::Result<()> {
    let mut config = Configuration::create()?;
    config.load_with_settings()?;

    match action {
        Some("start") => start_background(&config).await,
        Some("start-sync") => start_sync(&config).await,
        Some("stop") => stop(&config).await,
        Some("status") => status(&config).await,
        Some("logs") => show_logs(&config),
        Some("list") => list(&config).await,
        _ => {
            eprintln!("Unknown runner action. Run 'hapir runner --help' for usage.");
            Ok(())
        }
    }
}

async fn start_background(config: &Configuration) -> anyhow::Result<()> {
    // Validate required configuration before starting
    if config.cli_api_token.is_empty() {
        anyhow::bail!(
            "CLI_API_TOKEN is not configured. Run 'hapir auth login' or set the CLI_API_TOKEN environment variable."
        );
    }

    // Check if already running
    if let Some(port) = control_client::check_runner_alive(
        &config.runner_state_file,
        &config.runner_lock_file,
    )
    .await
    {
        eprintln!("Runner is already running on port {port}");
        return Ok(());
    }

    eprintln!("Starting runner in background...");
    common::spawn_runner_background()?;
    eprintln!("Runner started in background");
    Ok(())
}

async fn start_sync(config: &Configuration) -> anyhow::Result<()> {
    // Validate required configuration before starting
    if config.cli_api_token.is_empty() {
        anyhow::bail!(
            "CLI_API_TOKEN is not configured. Run 'hapir auth login' or set the CLI_API_TOKEN environment variable."
        );
    }

    // Acquire runner lock
    let _lock = persistence::acquire_runner_lock(&config.runner_lock_file, 5)
        .ok_or_else(|| anyhow::anyhow!("another runner instance is already starting"))?;

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    // Start control server
    let state = control_server::RunnerState::new(shutdown_tx);
    let port = control_server::start(state.clone()).await?;

    // Write runner state
    let start_time = chrono_now();
    let runner_local_state = persistence::RunnerLocalState {
        pid: std::process::id(),
        http_port: port,
        start_time: start_time.clone(),
        started_with_cli_version: env!("CARGO_PKG_VERSION").to_string(),
        started_with_cli_mtime_ms: None,
        last_heartbeat: None,
        runner_log_path: None,
    };
    persistence::write_runner_state(&config.runner_state_file, &runner_local_state)?;

    info!(port = port, pid = std::process::id(), "runner started");
    eprintln!("Runner started on port {port} (pid {})", std::process::id());

    // --- Hub WebSocket connectivity (best-effort) ---
    let ws_machine: Option<Arc<WsMachineClient>> = match connect_to_hub(config, port, &start_time, &state).await {
        Ok(ws) => Some(ws),
        Err(e) => {
            warn!("hub connection failed, running in local-only mode: {e}");
            None
        }
    };

    // Spawn heartbeat task
    let heartbeat_handle = if let Some(ref ws) = ws_machine {
        let ws = ws.clone();
        let runner_state_clone = state.clone();
        Some(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                // Prune dead sessions
                {
                    let mut sessions = runner_state_clone.sessions.lock().await;
                    sessions.retain(|_id, s| {
                        if let Some(pid) = s.pid {
                            persistence::is_process_alive(pid)
                        } else {
                            true
                        }
                    });
                }
                // Update heartbeat on hub
                if let Err(e) = ws.update_runner_state(|mut s| {
                    s["lastHeartbeat"] = json!(chrono_now());
                    s
                }).await {
                    warn!("heartbeat update failed: {e}");
                }
            }
        }))
    } else {
        None
    };

    // Wait for shutdown signal
    let _ = shutdown_rx.await;
    info!("runner shutting down");

    // Graceful shutdown: update hub state
    if let Some(ref ws) = ws_machine {
        if let Err(e) = ws.update_runner_state(|mut s| {
            s["status"] = json!("shutting-down");
            s
        }).await {
            warn!("failed to update shutdown state on hub: {e}");
        }
    }

    if let Some(handle) = heartbeat_handle {
        handle.abort();
    }

    if let Some(ws) = ws_machine {
        ws.shutdown().await;
    }

    // Clean up state
    persistence::clear_runner_state(&config.runner_state_file, &config.runner_lock_file);
    eprintln!("Runner stopped.");
    Ok(())
}

/// Connect to the hub via WebSocket, register RPC handlers, and send initial state.
async fn connect_to_hub(
    config: &Configuration,
    http_port: u16,
    start_time: &str,
    runner_state: &control_server::RunnerState,
) -> anyhow::Result<Arc<WsMachineClient>> {
    let machine_id = common::auth_and_setup_machine(config).await?;

    let ws = Arc::new(WsMachineClient::new(
        &config.api_url,
        &config.cli_api_token,
        &machine_id,
    ));

    // Register RPC handlers
    let rs = runner_state.clone();
    ws.register_rpc("spawn-happy-session", move |data| {
        let rs = rs.clone();
        Box::pin(async move {
            let req: crate::runner::types::SpawnSessionRequest =
                serde_json::from_value(data).unwrap_or_else(|_| {
                    crate::runner::types::SpawnSessionRequest {
                        directory: String::new(),
                        session_id: None,
                        session_type: None,
                        worktree_name: None,
                        flavor: None,
                    }
                });
            serde_json::to_value(control_server::do_spawn_session(&rs, req).await)
                .unwrap_or(json!({"error": "serialization failed"}))
        })
    }).await;

    let rs = runner_state.clone();
    ws.register_rpc("stop-session", move |data| {
        let rs = rs.clone();
        Box::pin(async move {
            let session_id = data.get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            control_server::do_stop_session(&rs, session_id).await
        })
    }).await;

    let rs = runner_state.clone();
    ws.register_rpc("stop-runner", move |_data| {
        let rs = rs.clone();
        Box::pin(async move {
            control_server::do_stop_runner(&rs).await
        })
    }).await;

    // Connect WebSocket (has built-in auto-reconnect)
    ws.connect().await;

    // Send machine metadata
    let meta = build_machine_metadata(config);
    ws.update_metadata(|_| serde_json::to_value(&meta).unwrap_or(json!({}))).await
        .map_err(|e| warn!("failed to send machine metadata: {e}")).ok();

    // Send runner state
    ws.update_runner_state(|_| json!({
        "status": "running",
        "pid": std::process::id(),
        "httpPort": http_port,
        "startedAt": start_time,
        "cliVersion": env!("CARGO_PKG_VERSION"),
    })).await
        .map_err(|e| warn!("failed to send runner state: {e}")).ok();

    info!(machine_id = %machine_id, "connected to hub via WebSocket");
    Ok(ws)
}

async fn stop(config: &Configuration) -> anyhow::Result<()> {
    match control_client::check_runner_alive(
        &config.runner_state_file,
        &config.runner_lock_file,
    )
    .await
    {
        Some(port) => {
            control_client::stop_runner(port).await?;
            eprintln!("Runner stop requested.");
        }
        None => {
            eprintln!("Runner is not running.");
        }
    }
    Ok(())
}

async fn status(config: &Configuration) -> anyhow::Result<()> {
    match control_client::check_runner_alive(
        &config.runner_state_file,
        &config.runner_lock_file,
    )
    .await
    {
        Some(port) => {
            let state = persistence::read_runner_state(&config.runner_state_file);
            if let Some(state) = state {
                eprintln!("Runner is running:");
                eprintln!("  PID:        {}", state.pid);
                eprintln!("  Port:       {port}");
                eprintln!("  Started:    {}", state.start_time);
                eprintln!("  Version:    {}", state.started_with_cli_version);
            } else {
                eprintln!("Runner is running on port {port}");
            }
        }
        None => {
            eprintln!("Runner is not running.");
        }
    }
    Ok(())
}

async fn list(config: &Configuration) -> anyhow::Result<()> {
    match control_client::check_runner_alive(
        &config.runner_state_file,
        &config.runner_lock_file,
    )
    .await
    {
        Some(port) => {
            let sessions = control_client::list_sessions(port).await?;
            if sessions.is_empty() {
                eprintln!("No active sessions.");
            } else {
                eprintln!("{} active session(s):", sessions.len());
                for s in &sessions {
                    eprintln!(
                        "  {} (pid: {}, dir: {}, by: {})",
                        s.session_id,
                        s.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                        s.directory,
                        s.started_by,
                    );
                }
            }
        }
        None => {
            eprintln!("Runner is not running.");
        }
    }
    Ok(())
}

fn show_logs(config: &Configuration) -> anyhow::Result<()> {
    if let Some(state) = persistence::read_runner_state(&config.runner_state_file) {
        if let Some(ref log_path) = state.runner_log_path {
            let path = std::path::Path::new(log_path);
            if path.exists() {
                let content = std::fs::read_to_string(path)?;
                // Show last 50 lines
                let lines: Vec<&str> = content.lines().collect();
                let start = lines.len().saturating_sub(50);
                for line in &lines[start..] {
                    println!("{line}");
                }
                return Ok(());
            }
            eprintln!("Log file not found: {log_path}");
            return Ok(());
        }
    }
    eprintln!("No runner logs available. Is the runner running?");
    Ok(())
}

/// Simple ISO-8601 timestamp without pulling in chrono.
fn chrono_now() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s-since-epoch", dur.as_secs())
}
