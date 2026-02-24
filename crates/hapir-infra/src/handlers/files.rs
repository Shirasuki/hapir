use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::debug;

use hapir_shared::rpc::files::{
    RpcReadFileRequest, RpcReadFileResponse, RpcWriteFileRequest, RpcWriteFileResponse,
};

use crate::rpc::RpcRegistry;
use crate::utils::path::validate_path;

pub async fn register_file_handlers(rpc: &(impl RpcRegistry + Sync), working_directory: &str) {
    let wd = Arc::new(working_directory.to_string());

    // readFile handler
    {
        let wd = wd.clone();
        rpc.register("readFile", move |params: Value| {
            let wd = wd.clone();
            async move {
                let mut response = RpcReadFileResponse::default();
                let req: RpcReadFileRequest = match serde_json::from_value(params) {
                    Ok(r) => r,
                    Err(_) => {
                        response.error = Some("Missing 'path' field".into());
                        return serde_json::to_value(response).unwrap();
                    }
                };

                if let Err(e) = validate_path(&req.path, &wd) {
                    response.error = Some(e);
                    return serde_json::to_value(response).unwrap();
                }

                let resolved = Path::new(wd.as_str()).join(&req.path);
                debug!(path = %resolved.display(), "readFile handler");

                match tokio::fs::read(&resolved).await {
                    Ok(bytes) => {
                        let content = BASE64.encode(&bytes);
                        response.success = true;
                        response.content = Some(content);
                        response.error = None;
                        serde_json::to_value(response).unwrap()
                    }
                    Err(e) => {
                        debug!(error = %e, "Failed to read file");
                        response.error = Some(format!("Failed to read file: {e}"));
                        serde_json::to_value(response).unwrap()
                    }
                }
            }
        })
        .await;
    }

    // writeFile handler
    {
        let wd = wd.clone();
        rpc.register("writeFile", move |params: Value| {
            let wd = wd.clone();
            async move {
                let mut response = RpcWriteFileResponse::default();
                let req: RpcWriteFileRequest = match serde_json::from_value(params) {
                    Ok(r) => r,
                    Err(_) => {
                        response.error = Some("Missing required fields".into());
                        return serde_json::to_value(response).unwrap();
                    },
                };

                if let Err(e) = validate_path(&req.path, &wd) {
                    response.error = Some(e);
                    return serde_json::to_value(response).unwrap();
                }

                let resolved = Path::new(wd.as_str()).join(&req.path);
                debug!(path = %resolved.display(), "writeFile handler");

                let bytes = match BASE64.decode(&req.content) {
                    Ok(b) => b,
                    Err(e) => {
                        response.error = Some(format!("Invalid base64 content: {e}"));
                        return serde_json::to_value(response).unwrap();
                    },
                };

                // Hash-based conflict detection
                if let Some(ref expected) = req.expected_hash {
                    match tokio::fs::read(&resolved).await {
                        Ok(existing) => {
                            let mut hasher = Sha256::new();
                            hasher.update(&existing);
                            let actual_hash = hex::encode(hasher.finalize());
                            if &actual_hash != expected {
                                response.error = Some(format!(
                                    "File hash mismatch. Expected: {expected}, Actual: {actual_hash}"
                                ));
                                return serde_json::to_value(response).unwrap();
                            }
                        }
                        Err(e) if e.kind() == ErrorKind::NotFound => {
                            response.error = Some("File does not exist but hash was provided".into());
                            return serde_json::to_value(response).unwrap();
                        }
                        Err(e) => {
                            response.error = Some(format!("Failed to read existing file: {e}"));
                            return serde_json::to_value(response).unwrap();
                        }
                    }
                } else {
                    match tokio::fs::metadata(&resolved).await {
                        Ok(_) => {
                            response.error = Some("File already exists but was expected to be new".into());
                            return serde_json::to_value(response).unwrap();
                        }
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => {
                            response.error = Some(format!("Failed to check file: {e}"));
                            return serde_json::to_value(response).unwrap();
                        }
                    }
                }

                if let Some(parent) = resolved.parent()
                    && let Err(e) = tokio::fs::create_dir_all(parent).await
                {
                    response.error = Some(format!("Failed to create directories: {e}"));
                    return serde_json::to_value(response).unwrap();
                }

                match tokio::fs::write(&resolved, &bytes).await {
                    Ok(()) => {
                        let mut hasher = Sha256::new();
                        hasher.update(&bytes);
                        let hash = hex::encode(hasher.finalize());
                        response.success = true;
                        response.hash = Some(hash);
                        response.error = None;
                        serde_json::to_value(response).unwrap()
                    }
                    Err(e) => {
                        debug!(error = %e, "Failed to write file");
                        response.error = Some(format!("Failed to write file: {e}"));
                        serde_json::to_value(response).unwrap()
                    }
                }
            }
        })
        .await;
    }
}
