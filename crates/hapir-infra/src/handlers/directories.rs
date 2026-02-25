use serde_json::Value;
use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tracing::debug;

use hapir_shared::rpc::directories::{
    RpcDirectoryEntry, RpcGetDirectoryTreeRequest, RpcGetDirectoryTreeResponse,
    RpcListDirectoryRequest, RpcListDirectoryResponse, RpcTreeNode,
};

use crate::rpc::RpcRegistry;
use crate::utils::path_security::validate_path;

pub async fn register_directory_handlers(rpc: &(impl RpcRegistry + Sync), working_directory: &str) {
    let wd = Arc::new(working_directory.to_string());

    // listDirectory handler
    {
        let wd = wd.clone();
        rpc.register_rpc("listDirectory", move |params: Value| {
            let wd = wd.clone();
            async move {
                let req = serde_json::from_value(params)
                    .unwrap_or(RpcListDirectoryRequest { path: ".".into() });
                let mut response = RpcListDirectoryResponse::default();

                if let Err(e) = validate_path(&req.path, &wd) {
                    response.success = false;
                    response.entries = None;
                    response.error = Some(e);
                    return serde_json::to_value(response).unwrap();
                }

                let resolved = Path::new(wd.as_str()).join(&req.path);
                debug!(path = %resolved.display(), "listDirectory handler");

                let mut read_dir = match tokio::fs::read_dir(&resolved).await {
                    Ok(rd) => rd,
                    Err(e) => {
                        debug!(error = %e, "Failed to list directory");
                        response.success = false;
                        response.entries = None;
                        response.error = Some(format!("Failed to list directory: {e}"));
                        return serde_json::to_value(response).unwrap();
                    }
                };

                let mut entries = Vec::new();
                while let Ok(Some(entry)) = read_dir.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let file_type = entry.file_type().await.ok();

                    let entry_type = match &file_type {
                        Some(ft) if ft.is_dir() => "directory",
                        Some(ft) if ft.is_file() => "file",
                        _ => "other",
                    };

                    let mut size = None;
                    let mut modified = None;

                    if file_type.as_ref().is_some_and(|ft| !ft.is_symlink())
                        && let Ok(meta) = entry.metadata().await
                    {
                        size = Some(meta.len());
                        if let Ok(m) = meta.modified()
                            && let Ok(dur) = m.duration_since(UNIX_EPOCH)
                        {
                            modified = Some(dur.as_millis() as u64);
                        }
                    }

                    entries.push(RpcDirectoryEntry {
                        name,
                        entry_type: entry_type.into(),
                        size,
                        modified,
                    });
                }

                // Sort: directories first, then alphabetical
                entries.sort_by(|a, b| {
                    let a_is_dir = a.entry_type == "directory";
                    let b_is_dir = b.entry_type == "directory";
                    match (a_is_dir, b_is_dir) {
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => a.name.cmp(&b.name),
                    }
                });

                response.success = true;
                response.entries = Some(entries);
                response.error = None;

                serde_json::to_value(response).unwrap()
            }
        })
        .await;
    }

    // getDirectoryTree handler
    {
        let wd = wd.clone();
        rpc.register_rpc("getDirectoryTree", move |params: Value| {
            let wd = wd.clone();
            async move {
                let req = serde_json::from_value(params)
                    .unwrap_or(RpcGetDirectoryTreeRequest {
                        path: ".".into(),
                        max_depth: 3,
                    });
                let mut response = RpcGetDirectoryTreeResponse::default();

                if req.max_depth < 0 {
                    response.success = false;
                    response.tree = None;
                    response.error = Some("maxDepth must be non-negative".into());
                    return serde_json::to_value(response).unwrap();
                }

                if let Err(e) = validate_path(&req.path, &wd) {
                    response.success = false;
                    response.tree = None;
                    response.error = Some(e);
                    return serde_json::to_value(response).unwrap();
                }

                let resolved = Path::new(wd.as_str()).join(&req.path);
                debug!(path = %resolved.display(), max_depth = req.max_depth, "getDirectoryTree handler");

                let base_name = resolved
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| resolved.to_string_lossy().to_string());

                match build_tree(&resolved, &base_name, 0, req.max_depth as usize).await {
                    Some(tree) => {
                        response.success = true;
                        response.tree = Some(tree);
                        response.error = None;
                        serde_json::to_value(response).unwrap()
                    },
                    None => serde_json::to_value(RpcGetDirectoryTreeResponse {
                        success: false,
                        tree: None,
                        error: Some("Failed to access the specified path".into()),
                    })
                    .unwrap(),
                }
            }
        })
        .await;
    }
}

async fn build_tree(
    path: &Path,
    name: &str,
    depth: usize,
    max_depth: usize,
) -> Option<RpcTreeNode> {
    let meta = tokio::fs::symlink_metadata(path).await.ok()?;

    let node_type = if meta.is_dir() { "directory" } else { "file" };

    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|dur| dur.as_millis() as u64);

    let children = if meta.is_dir() && depth < max_depth {
        let mut read_dir = tokio::fs::read_dir(path).await.ok()?;
        let mut children = Vec::new();

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            let child_name = entry.file_name().to_string_lossy().to_string();
            let child_path = entry.path();
            if let Some(child_node) =
                Box::pin(build_tree(&child_path, &child_name, depth + 1, max_depth)).await
            {
                children.push(child_node);
            }
        }

        children.sort_by(|a, b| {
            let a_is_dir = a.node_type == "directory";
            let b_is_dir = b.node_type == "directory";
            match (a_is_dir, b_is_dir) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });

        Some(children)
    } else {
        None
    };

    Some(RpcTreeNode {
        name: name.into(),
        path: path.to_string_lossy().into(),
        node_type: node_type.into(),
        size: meta.len(),
        modified,
        children,
    })
}
