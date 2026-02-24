use anyhow::Result;
use clap::Parser;
use std::env;
use tracing::debug;
use codex::run::run_codex;
use crate::commands::common;
use crate::modules::codex;
use crate::modules::codex::run::CodexStartOptions;
use crate::agent::session_base::SessionMode;
use hapir_infra::config::CliConfiguration;
use hapir_shared::schemas::SessionStartedBy;

/// Parsed arguments for the codex command.
#[derive(Parser, Debug, Default)]
#[command(name = "codex")]
pub struct CodexArgs {
    /// Resume an existing session
    pub resume: Option<String>,

    /// What started this session
    #[arg(long)]
    pub started_by: Option<String>,

    /// Starting mode for the session
    #[arg(long)]
    pub hapir_starting_mode: Option<String>,

    /// Bypass permission prompts
    #[arg(long)]
    pub yolo: bool,

    /// Model to use
    #[arg(long)]
    pub model: Option<String>,
}

/// Run the codex command.
pub async fn run(args: CodexArgs) -> Result<()> {
    debug!(?args, "codex command starting");

    let mut config = CliConfiguration::new()?;
    let runner_port = common::full_init(&mut config).await?;

    let working_directory = env::current_dir()?.to_string_lossy().to_string();

    let started_by = match args.started_by.as_deref() {
        Some("runner") => SessionStartedBy::Runner,
        _ => SessionStartedBy::Terminal,
    };
    let starting_mode = args.hapir_starting_mode.as_deref().map(|s| match s {
        "remote" => SessionMode::Remote,
        "local" => SessionMode::Local,
        other => panic!("Unsupported starting mode: {}", other),
    });

    run_codex(CodexStartOptions {
        working_directory,
        runner_port,
        started_by,
        starting_mode,
        model: args.model,
        yolo: args.yolo,
        resume: args.resume,
    })
    .await
}
