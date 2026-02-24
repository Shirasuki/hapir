use anyhow::Result;
use clap::Parser;
use std::env;
use tracing::debug;

use hapir_shared::modes::SessionMode;
use crate::commands::common;
use crate::modules::claude::run::{ClaudeStartOptions, run_claude};
use crate::modules::claude::version_check::check_claude_version;
use hapir_infra::config::CliConfiguration;
use hapir_shared::modes::PermissionMode;
use hapir_shared::schemas::SessionStartedBy;

/// Parsed arguments for the claude (default) command.
#[derive(Parser, Debug, Default)]
#[command(name = "claude")]
pub struct ClaudeArgs {
    /// Starting mode for the session
    #[arg(long)]
    pub hapir_starting_mode: Option<String>,

    /// Bypass permission prompts
    #[arg(long)]
    pub yolo: bool,

    /// Skip all permission checks (dangerous)
    #[arg(long)]
    pub dangerously_skip_permissions: bool,

    /// Model to use
    #[arg(long)]
    pub model: Option<String>,

    /// What started this session
    #[arg(long)]
    pub started_by: Option<String>,

    /// Extra arguments passed through to claude
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub passthrough_args: Vec<String>,
}

/// Run the default (claude) command.
///
/// Mirrors the TypeScript `claude.ts` entry point: parse args, initialize
/// token, register machine, ensure runner, then hand off to `run_claude`.
pub async fn run(args: ClaudeArgs) -> Result<()> {
    debug!(?args, "claude command starting");

    check_claude_version()?;

    let mut config = CliConfiguration::new()?;

    // Map --yolo / --dangerously-skip-permissions to permission mode
    let permission_mode = if args.dangerously_skip_permissions || args.yolo {
        Some(PermissionMode::BypassPermissions)
    } else {
        None
    };

    let working_directory = env::current_dir()?.to_string_lossy().to_string();
    // Full init: token, machine, runner
    let runner_port = common::full_init(&mut config).await?;

    let started_by = match args.started_by.as_deref() {
        Some("runner") => SessionStartedBy::Runner,
        _ => SessionStartedBy::Terminal,
    };
    let starting_mode = args.hapir_starting_mode.as_deref().map(|s| match s {
        "remote" => SessionMode::Remote,
        "local" => SessionMode::Local,
        other => panic!("Unsupported starting mode: {}", other),
    });

    let options = ClaudeStartOptions {
        working_directory,
        model: args.model,
        permission_mode,
        starting_mode,
        should_start_runner: Some(true),
        claude_env_vars: None,
        claude_args: if args.passthrough_args.is_empty() {
            None
        } else {
            Some(args.passthrough_args)
        },
        started_by,
        runner_port,
    };

    run_claude(options, &config).await
}
