use crate::api::ApiClient;
use crate::config::CliConfiguration;
use crate::persistence;
use crate::utils::machine::build_machine_metadata;
use anyhow::{Context, Result};
use hapir_shared::schemas::MachineRunnerState;
use tracing::info;
use uuid::Uuid;

/// Register (or confirm) the machine with the hub API, optionally sending
/// initial runner state. Returns the machine_id.
pub async fn auth_and_setup_machine_with_state(
    config: &CliConfiguration,
    runner_state: Option<&MachineRunnerState>,
) -> Result<String> {
    let api = ApiClient::new(config)?;
    let settings = persistence::read_settings(&config.settings_file)?;

    let machine_id = if let Some(ref id) = settings.machine_id {
        id.clone()
    } else {
        let id = format!("mach_{}", Uuid::new_v4());
        persistence::update_settings(&config.settings_file, |s| {
            s.machine_id = Some(id.clone());
        })?;
        id
    };

    info!(machine_id = %machine_id, "registering machine");
    let machine_meta = build_machine_metadata(&config.home_dir);
    api.get_or_create_machine(&machine_id, &machine_meta, runner_state)
        .await
        .context("failed to register machine with hub")?;

    persistence::update_settings(&config.settings_file, |s| {
        s.machine_id_confirmed_by_server = Some(true);
    })?;

    Ok(machine_id)
}
