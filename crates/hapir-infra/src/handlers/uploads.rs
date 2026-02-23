use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::Value;
use tracing::debug;

use hapir_shared::rpc::uploads::{
    RpcDeleteUploadRequest, RpcDeleteUploadResponse, RpcUploadFileRequest, RpcUploadFileResponse,
};

use crate::rpc::RpcRegistry;

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

fn upload_dir(session_id: &str) -> PathBuf {
    std::env::temp_dir().join("hapir-blobs").join(session_id)
}

pub async fn cleanup_upload_dir(session_id: &str) {
    let dir = upload_dir(session_id);
    if dir.exists()
        && let Err(e) = tokio::fs::remove_dir_all(&dir).await
    {
        debug!("Failed to cleanup upload dir {}: {}", dir.display(), e);
    }
}

fn sanitize_filename(filename: &str) -> String {
    let sanitized: String = filename
        .replace(['/', '\\'], "_")
        .replace("..", "_")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_");
    let truncated = if sanitized.len() > 255 {
        sanitized[..255].to_string()
    } else {
        sanitized
    };
    if truncated.is_empty() {
        "upload".to_string()
    } else {
        truncated
    }
}

pub async fn register_upload_handlers(rpc: &(impl RpcRegistry + Sync), _working_directory: &str) {
    // uploadFile handler
    rpc.register("uploadFile", move |params: Value| async move {
        let mut response = RpcUploadFileResponse::default();
        let req = match serde_json::from_value::<RpcUploadFileRequest>(params) {
            Ok(r) if !r.filename.is_empty() && !r.content.is_empty() => r,
            _ => {
                response.error = Some("Filename and content are required".to_string());
                return serde_json::to_value(response).unwrap();
            }
        };

        let estimated = (req.content.len() * 3) / 4;
        if estimated > MAX_UPLOAD_BYTES {
            return {
                response.error = Some("File too large (max 50MB)".to_string());
                serde_json::to_value(response).unwrap()
            };
        }

        let bytes = match BASE64.decode(&req.content) {
            Ok(b) => b,
            Err(e) => {
                response.error = Some(format!("Invalid base64: {e}"));
                return serde_json::to_value(response).unwrap();
            }
        };
        if bytes.len() > MAX_UPLOAD_BYTES {
            response.error = Some("File too large (max 50MB)".to_string());
            return serde_json::to_value(response).unwrap();
        }

        let dir = upload_dir(&req.session_id);
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            response.error = Some(format!("Failed to create upload dir: {e}"));
            return serde_json::to_value(response).unwrap();
        }

        let sanitized = sanitize_filename(&req.filename);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let file_path = dir.join(format!("{timestamp}-{sanitized}"));

        match tokio::fs::write(&file_path, &bytes).await {
            Ok(()) => {
                debug!(path = %file_path.display(), "File uploaded");
                response.success = true;
                response.path = Some(file_path.to_string_lossy().into());
                response.error = None;
                serde_json::to_value(response).unwrap()
            }
            Err(e) => {
                response.error = Some(format!("Failed to write file: {e}"));
                serde_json::to_value(response).unwrap()
            }
        }
    })
    .await;

    // deleteUpload handler
    rpc.register("deleteUpload", move |params: Value| async move {
        let mut resp = RpcDeleteUploadResponse::default();
        let req = match serde_json::from_value::<RpcDeleteUploadRequest>(params) {
            Ok(r) if !r.path.trim().is_empty() => r,
            _ => {
                resp.error = Some("Path is required".to_string());
                return serde_json::to_value(resp).unwrap();
            }
        };

        let path = Path::new(req.path.trim());
        let dir = upload_dir(&req.session_id);
        let ok = path.canonicalize().ok().is_some_and(|resolved| {
            dir.canonicalize()
                .ok()
                .is_some_and(|resolved_dir| resolved.starts_with(&resolved_dir))
        });
        if !ok {
            resp.error = Some("Invalid upload path".to_string());
            return serde_json::to_value(resp).unwrap();
        }

        match tokio::fs::remove_file(path).await {
            Ok(()) => {
                resp.success = true;
                resp.error = None;
                serde_json::to_value(resp).unwrap()
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                resp.success = true;
                resp.error = None;
                serde_json::to_value(resp).unwrap()
            }
            Err(e) => {
                resp.error = Some(format!("Failed to delete file: {e}"));
                serde_json::to_value(resp).unwrap()
            }
        }
    })
    .await;
}
