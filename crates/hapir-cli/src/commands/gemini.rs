use hapir_shared::modes::SessionMode;
use crate::commands::common;
use crate::modules::gemini;
use crate::modules::gemini::run::GeminiStartOptions;
use anyhow::Result;
use clap::Parser;
use gemini::run::run_gemini;
use hapir_infra::config::CliConfiguration;
use hapir_shared::schemas::SessionStartedBy;
use std::env;
use tracing::debug;

/// Parsed arguments for the gemini command.
#[derive(Parser, Debug, Default)]
#[command(name = "gemini")]
pub struct GeminiArgs {
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

/// Run the gemini command.
pub async fn run(args: GeminiArgs) -> Result<()> {
    debug!(?args, "gemini command starting");

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

    run_gemini(
        GeminiStartOptions {
            working_directory,
            runner_port,
            started_by,
            starting_mode,
        },
        &config,
    )
    .await
}
