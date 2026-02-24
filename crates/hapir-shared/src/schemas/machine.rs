use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MachineRunnerStatus {
    Offline,
    Running,
    ShuttingDown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineRunnerState {
    pub status: MachineRunnerStatus,
    pub pid: u32,
    pub http_port: u16,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_requested_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_source: Option<String>,
}

/// Machine-level metadata sent when registering a machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HapirMachineMetadata {
    pub host: String,
    pub platform: String,
    pub happy_cli_version: String,
    pub home_dir: String,
    pub happy_home_dir: String,
    pub happy_lib_dir: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_state_serde_roundtrip() {
        let state = MachineRunnerState {
            status: MachineRunnerStatus::Running,
            pid: 1234,
            http_port: 3006,
            started_at: 1700000000000,
            shutdown_requested_at: None,
            shutdown_source: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: MachineRunnerState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pid, 1234);
        assert_eq!(back.status, MachineRunnerStatus::Running);
    }

    #[test]
    fn runner_status_kebab_case() {
        let json = serde_json::to_string(&MachineRunnerStatus::ShuttingDown).unwrap();
        assert_eq!(json, "\"shutting-down\"");
        let back: MachineRunnerStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MachineRunnerStatus::ShuttingDown);
    }
}
