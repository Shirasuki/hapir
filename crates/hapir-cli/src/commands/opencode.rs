use std::env;
use anyhow::Result;
use clap::Parser;
use tracing::debug;

use crate::commands::common;
use hapir_infra::config::CliConfiguration;
use opencode::run::run_opencode;
use crate::modules::opencode;
use crate::modules::opencode::run::OpencodeStartOptions;
use crate::agent::session_base::SessionMode;
use hapir_shared::schemas::SessionStartedBy;

/// Parsed arguments for the opencode command.
#[derive(Parser, Debug, Default)]
#[command(name = "opencode")]
pub struct OpencodeArgs {
    /// What started this session
    #[arg(long)]
    pub started_by: Option<String>,

    /// Starting mode for the session
    #[arg(long)]
    pub hapir_starting_mode: Option<String>,

    /// Bypass permission prompts
    #[arg(long)]
    pub yolo: bool,

    /// Resume an existing session
    #[arg(long)]
    pub resume: Option<String>,
}

/// Run the opencode command.
pub async fn run(args: OpencodeArgs) -> Result<()> {
    debug!(?args, "opencode command starting");

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

    run_opencode(OpencodeStartOptions {
        working_directory,
        runner_port,
        started_by,
        starting_mode,
        resume: args.resume,
    })
    .await
}
