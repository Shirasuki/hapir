use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

use hapir_shared::rpc::bash::RpcCommandResponse;
use hapir_shared::rpc::git::{
    RpcGitDiffFileRequest, RpcGitDiffNumstatRequest, RpcGitStatusRequest,
};

use crate::rpc::RpcRegistry;
use crate::utils::path::validate_path;

async fn run_git_command(args: &[&str], cwd: &str, timeout_ms: u64) -> Value {
    let result = timeout(
        Duration::from_millis(timeout_ms),
        Command::new("git").args(args).current_dir(cwd).output(),
    )
    .await;

    let mut response = RpcCommandResponse::default();

    match result {
        Ok(Ok(output)) => {
            let code = output.status.code().unwrap_or(1);
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                response.success = true;
                response.stdout = Some(stdout);
                response.stderr = Some(stderr);
                response.exit_code = Some(code);
                response.error = None;
            } else {
                response.success = false;
                response.stdout = Some(stdout);
                response.stderr = Some(stderr.clone());
                response.exit_code = Some(code);
                response.error = Some(stderr);
            }
        }
        Ok(Err(e)) => {
            response.success = false;
            response.stdout = Some(String::new());
            response.stderr = Some(e.to_string());
            response.exit_code = Some(1);
            response.error = Some(e.to_string());
        }
        Err(_) => {
            response.success = false;
            response.stdout = Some(String::new());
            response.stderr = Some(String::new());
            response.exit_code = Some(-1);
            response.error = Some("Command timed out".into());
        }
    }
    serde_json::to_value(response).unwrap()
}

#[inline]
fn resolve_cwd(cwd: Option<&str>, wd: &str) -> Result<String, String> {
    let cwd = cwd.unwrap_or(wd);
    if let Err(e) = validate_path(cwd, wd) {
        return Err(e);
    }
    Ok(cwd.to_string())
}

pub async fn register_git_handlers(rpc: &(impl RpcRegistry + Sync), working_directory: &str) {
    let wd = Arc::new(working_directory.to_string());

    // git-status
    {
        let wd = wd.clone();
        rpc.register("git-status", move |params: Value| {
            let wd = wd.clone();
            async move {
                let req: RpcGitStatusRequest = serde_json::from_value(params).unwrap_or_default();
                let cwd = match resolve_cwd(req.cwd.as_deref(), &wd) {
                    Ok(c) => c,
                    Err(e) => {
                        let mut response = RpcCommandResponse::default();
                        response.error = Some(e);
                        return serde_json::to_value(response).unwrap();
                    },
                };
                let timeout = req.timeout.unwrap_or(10_000);
                debug!(cwd = %cwd, "git-status handler");
                run_git_command(
                    &[
                        "status",
                        "--porcelain=v2",
                        "--branch",
                        "--untracked-files=all",
                    ],
                    &cwd,
                    timeout,
                )
                .await
            }
        })
        .await;
    }

    // git-diff-numstat
    {
        let wd = wd.clone();
        rpc.register("git-diff-numstat", move |params: Value| {
            let wd = wd.clone();
            async move {
                let req: RpcGitDiffNumstatRequest =
                    serde_json::from_value(params).unwrap_or_default();
                let cwd = match resolve_cwd(req.cwd.as_deref(), &wd) {
                    Ok(c) => c,
                    Err(e) => {
                        let mut response = RpcCommandResponse::default();
                        response.error = Some(e);
                        return serde_json::to_value(response).unwrap();
                    },
                };
                let timeout = req.timeout.unwrap_or(10_000);
                let staged = req.staged.unwrap_or(false);
                debug!(cwd = %cwd, staged, "git-diff-numstat handler");

                let args: Vec<&str> = if staged {
                    vec!["diff", "--cached", "--numstat"]
                } else {
                    vec!["diff", "--numstat"]
                };
                run_git_command(&args, &cwd, timeout).await
            }
        })
        .await;
    }

    // git-diff-file
    {
        let wd = wd.clone();
        rpc.register("git-diff-file", move |params: Value| {
            let wd = wd.clone();
            async move {
                let mut response = RpcCommandResponse::default();
                let req: RpcGitDiffFileRequest = match serde_json::from_value(params) {
                    Ok(r) => r,
                    Err(_) => {
                        response.error = Some("Missing 'filePath' field".to_string());
                        return serde_json::to_value(response).unwrap();
                    }
                };
                let cwd = match resolve_cwd(req.cwd.as_deref(), &wd) {
                    Ok(c) => c,
                    Err(e) => {
                        response.error = Some(e);
                        return serde_json::to_value(response).unwrap();
                    }
                };
                if let Err(e) = validate_path(&req.file_path, &wd) {
                    response.error = Some(e);
                    return serde_json::to_value(response).unwrap();
                }
                let timeout = req.timeout.unwrap_or(10_000);
                let staged = req.staged.unwrap_or(false);
                debug!(cwd = %cwd, file_path = %req.file_path, staged, "git-diff-file handler");

                let args: Vec<&str> = if staged {
                    vec!["diff", "--cached", "--no-ext-diff", "--", &req.file_path]
                } else {
                    vec!["diff", "--no-ext-diff", "--", &req.file_path]
                };
                run_git_command(&args, &cwd, timeout).await
            }
        })
        .await;
    }
}
