use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use tracing::debug;

use hapir_shared::rpc::bash::{RpcBashRequest, RpcCommandResponse};

use crate::rpc::RpcRegistry;
use crate::utils::path::validate_path;
use crate::utils::shell::{default_shell, shell_command_flag};

pub async fn register_bash_handlers(rpc: &(impl RpcRegistry + Sync), working_directory: &str) {
    let wd = working_directory.to_string();

    rpc.register("bash", move |params: Value| {
        let wd = wd.clone();
        async move {
            let mut response = RpcCommandResponse::default();

            let req: RpcBashRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(_) => {
                    response.error = Some("Missing 'command' field".into());
                    return serde_json::to_value(response).unwrap();
                }
            };

            let timeout_ms = req.timeout.unwrap_or(30_000);

            if let Some(ref cwd) = req.cwd
                && let Err(e) = validate_path(cwd, &wd)
            {
                response.error = Some(e);
                return serde_json::to_value(response).unwrap();
            }

            let cwd = req.cwd.unwrap_or_else(|| wd.to_string());

            debug!(command = %req.command, cwd = %cwd, "bash handler");

            let result = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                run_command(&req.command, &cwd),
            )
            .await;

            match result {
                Ok(Ok(output)) => {
                    let code = output.status.code().unwrap_or(1);
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    response.success = output.status.success();
                    response.stdout = Some(stdout.clone());
                    response.stderr = Some(stderr.clone());
                    response.exit_code = Some(code);
                    serde_json::to_value(response).unwrap()
                }
                Ok(Err(e)) => {
                    response.success = false;
                    response.stdout = Some(String::new());
                    response.stderr = Some(e.to_string());
                    response.exit_code = Some(1);
                    response.error = Some(e.to_string());
                    serde_json::to_value(response).unwrap()
                }
                Err(_) => {
                    response.success = false;
                    response.stdout = Some(String::new());
                    response.stderr = Some(String::new());
                    response.exit_code = Some(-1);
                    response.error = Some("Command timed out".into());
                    serde_json::to_value(response).unwrap()
                }
            }
        }
    })
    .await;
}

async fn run_command(command: &str, cwd: &str) -> std::io::Result<std::process::Output> {
    let shell = default_shell();
    let flag = shell_command_flag(&shell);
    Command::new(&shell)
        .arg(flag)
        .arg(command)
        .current_dir(cwd)
        .output()
        .await
}
