use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::get,
};
use serde::Deserialize;

use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::{FileSearchData, FileSearchItem};
use hapir_shared::frontend::rpc::bash::RpcCommandResponse;
use hapir_shared::frontend::rpc::directories::RpcListDirectoryResponse;
use hapir_shared::frontend::rpc::files::RpcReadFileResponse;

use crate::web::AppState;
use crate::web::middleware::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions/{id}/git-status", get(git_status))
        .route("/sessions/{id}/git-diff-numstat", get(git_diff_numstat))
        .route("/sessions/{id}/git-diff-file", get(git_diff_file))
        .route("/sessions/{id}/file", get(get_file))
        .route("/sessions/{id}/files", get(list_files))
        .route("/sessions/{id}/directory", get(list_directory))
}

/// Helper to resolve session access and extract the session path.
async fn resolve_session_and_path(
    state: &AppState,
    auth: &AuthContext,
    raw_id: &str,
) -> Result<(String, String), ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(raw_id, &auth.namespace)
        .await?;

    let session_path = session
        .metadata
        .as_ref()
        .map(|m| m.path.clone())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::Internal("Session path not available".into()))?;

    Ok((session_id, session_path))
}

fn rpc_error(err: anyhow::Error) -> ApiError {
    ApiError::Internal(err.to_string())
}

// --- Handlers ---

async fn git_status(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<RpcCommandResponse>>, ApiError> {
    let (session_id, session_path) = resolve_session_and_path(&state, &auth, &id).await?;

    let resp = state
        .sync_engine
        .get_git_status(&session_id, Some(&session_path))
        .await
        .map_err(rpc_error)?;

    Ok(Json(ApiResponse::ok(resp)))
}

#[derive(Deserialize)]
struct DiffNumstatQuery {
    staged: Option<String>,
}

async fn git_diff_numstat(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<DiffNumstatQuery>,
) -> Result<Json<ApiResponse<RpcCommandResponse>>, ApiError> {
    let (session_id, session_path) = resolve_session_and_path(&state, &auth, &id).await?;
    let staged = parse_bool_param(query.staged.as_deref());

    let resp = state
        .sync_engine
        .get_git_diff_numstat(&session_id, Some(&session_path), staged)
        .await
        .map_err(rpc_error)?;

    Ok(Json(ApiResponse::ok(resp)))
}

#[derive(Deserialize)]
struct DiffFileQuery {
    path: String,
    staged: Option<String>,
}

async fn git_diff_file(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    query: Result<Query<DiffFileQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ApiResponse<RpcCommandResponse>>, ApiError> {
    let Query(query) = match query {
        Ok(q) => q,
        Err(_) => {
            return Err(ApiError::BadRequest("Invalid file path".into()));
        }
    };

    if query.path.is_empty() {
        return Err(ApiError::BadRequest("Invalid file path".into()));
    }

    let (session_id, session_path) = resolve_session_and_path(&state, &auth, &id).await?;
    let staged = parse_bool_param(query.staged.as_deref());

    let resp = state
        .sync_engine
        .get_git_diff_file(&session_id, Some(&session_path), &query.path, staged)
        .await
        .map_err(rpc_error)?;

    Ok(Json(ApiResponse::ok(resp)))
}

#[derive(Deserialize)]
struct FilePathQuery {
    path: String,
}

async fn get_file(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    query: Result<Query<FilePathQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ApiResponse<RpcReadFileResponse>>, ApiError> {
    let Query(query) = match query {
        Ok(q) => q,
        Err(_) => {
            return Err(ApiError::BadRequest("Invalid file path".into()));
        }
    };

    if query.path.is_empty() {
        return Err(ApiError::BadRequest("Invalid file path".into()));
    }

    let (session_id, _session_path) = resolve_session_and_path(&state, &auth, &id).await?;

    let resp = state
        .sync_engine
        .read_session_file(&session_id, &query.path)
        .await
        .map_err(rpc_error)?;

    Ok(Json(ApiResponse::ok(resp)))
}

#[derive(Deserialize)]
struct FileSearchQuery {
    query: Option<String>,
    limit: Option<usize>,
}

async fn list_files(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<FileSearchQuery>,
) -> Result<Json<ApiResponse<FileSearchData>>, ApiError> {
    let (session_id, session_path) = resolve_session_and_path(&state, &auth, &id).await?;

    let search_query = query.query.as_deref().map(|q| q.trim()).unwrap_or("");
    let limit = query.limit.unwrap_or(200).clamp(1, 500);

    let mut args: Vec<String> = vec!["--files".to_string()];
    if !search_query.is_empty() {
        args.push("--iglob".to_string());
        args.push(format!("*{search_query}*"));
    }

    let resp = state
        .sync_engine
        .run_ripgrep(&session_id, &args, Some(&session_path))
        .await
        .map_err(rpc_error)?;

    if !resp.ok {
        return Err(ApiError::Internal(
            resp.error
                .unwrap_or_else(|| "Failed to list files".to_string()),
        ));
    }

    let stdout = resp.stdout.unwrap_or_default();
    let files: Vec<FileSearchItem> = stdout
        .split('\n')
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .take(limit)
        .map(|full_path| {
            let parts: Vec<&str> = full_path.split('/').collect();
            let file_name = parts.last().copied().unwrap_or(full_path).to_string();
            let file_path = if parts.len() > 1 {
                parts[..parts.len() - 1].join("/")
            } else {
                String::new()
            };
            FileSearchItem {
                file_name,
                file_path,
                full_path: full_path.to_string(),
                file_type: "file".to_string(),
            }
        })
        .collect();

    Ok(Json(ApiResponse::ok(FileSearchData { files })))
}

#[derive(Deserialize)]
struct DirectoryQuery {
    path: Option<String>,
}

async fn list_directory(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<DirectoryQuery>,
) -> Result<Json<ApiResponse<RpcListDirectoryResponse>>, ApiError> {
    let (session_id, _session_path) = resolve_session_and_path(&state, &auth, &id).await?;
    let path = query.path.as_deref().unwrap_or("");

    let resp = state
        .sync_engine
        .list_directory(&session_id, path)
        .await
        .map_err(rpc_error)?;

    Ok(Json(ApiResponse::ok(resp)))
}

fn parse_bool_param(value: Option<&str>) -> Option<bool> {
    match value {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}
