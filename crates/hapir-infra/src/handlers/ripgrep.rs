use std::sync::Arc;

use serde_json::Value;
use tokio::process::Command;
use tracing::debug;

use hapir_shared::rpc::bash::RpcCommandResponse;
use hapir_shared::rpc::ripgrep::RpcRipgrepRequest;

use crate::rpc::RpcRegistry;
use crate::utils::path_security::validate_path;

pub async fn register_ripgrep_handlers(rpc: &(impl RpcRegistry + Sync), working_directory: &str) {
    let wd = Arc::new(working_directory.to_string());

    rpc.register("ripgrep", move |params: Value| {
        let wd = wd.clone();
        async move {
            let mut response = RpcCommandResponse::default();
            let req: RpcRipgrepRequest = match serde_json::from_value(params) {
                Ok(r) => r,
                Err(_) => {
                    response.error = Some("Missing 'args' field".into());
                    return serde_json::to_value(response).unwrap();
                }
            };

            if let Some(ref cwd) = req.cwd
                && let Err(e) = validate_path(cwd, &wd)
            {
                response.error = Some(e);
                return serde_json::to_value(response).unwrap();
            }

            let cwd = req.cwd.unwrap_or_else(|| wd.to_string());
            debug!(args = ?req.args, cwd = %cwd, "ripgrep handler");

            let result = Command::new("rg")
                .args(&req.args)
                .current_dir(&cwd)
                .output()
                .await;

            match result {
                Ok(output) => {
                    let code = output.status.code().unwrap_or(1);
                    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                    response.success = true;
                    response.exit_code = Some(code);
                    response.stdout = Some(stdout);
                    response.stderr = Some(stderr);
                    response.error = None;

                    serde_json::to_value(response).unwrap()
                }
                Err(e) => {
                    debug!(error = %e, "Failed to run ripgrep");

                    response.success = false;
                    response.error = Some(format!("Failed to run ripgrep: {e}"));

                    serde_json::to_value(response).unwrap()
                }
            }
        }
    })
    .await;
}
