use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing::{delete, get, patch, post},
};
use serde::Deserialize;
use serde_json::Value;
use std::cmp::Ordering;

use hapir_shared::common::modes::{
    AgentFlavor, ModelMode, PermissionMode, is_model_mode_allowed_for_flavor,
    is_permission_mode_allowed_for_flavor, permission_modes_for_flavor,
};
use hapir_shared::common::summary::to_session_summary;
use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::{ResumeData, SessionData, SessionsData};
use hapir_shared::frontend::rpc::uploads::{RpcDeleteUploadResponse, RpcUploadFileResponse};

use crate::sync::{ResumeSessionErrorCode, ResumeSessionResult};
use crate::web::AppState;
use crate::web::middleware::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", get(get_session))
        .route("/sessions/{id}", delete(delete_session))
        .route("/sessions/{id}", patch(update_session))
        .route("/sessions/{id}/resume", post(resume_session))
        .route("/sessions/{id}/archive", post(archive_session))
        .route("/sessions/{id}/abort", post(abort_session))
        .route("/sessions/{id}/switch", post(switch_session))
        .route("/sessions/{id}/permission-mode", post(set_permission_mode))
        .route("/sessions/{id}/model", post(set_model))
        .route("/sessions/{id}/slash-commands", get(list_slash_commands))
        .route("/sessions/{id}/skills", get(list_skills))
        .route("/sessions/{id}/upload", post(upload_file))
        .route("/sessions/{id}/upload/delete", post(delete_upload_file))
}

fn pending_count(session: &hapir_shared::common::session::Session) -> usize {
    session
        .agent_state
        .as_ref()
        .and_then(|s| s.requests.as_ref())
        .map(|r| r.len())
        .unwrap_or(0)
}

fn parse_flavor(session: &hapir_shared::common::session::Session) -> Option<AgentFlavor> {
    session.metadata.as_ref().and_then(|m| m.flavor)
}

async fn list_sessions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<ApiResponse<SessionsData>>, ApiError> {
    let mut sessions = state
        .sync_engine
        .get_sessions_by_namespace(&auth.namespace)
        .await;

    sessions.sort_by(|a, b| {
        if a.active != b.active {
            return if a.active {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        if a.active {
            let a_pending = pending_count(a);
            let b_pending = pending_count(b);
            if a_pending != b_pending {
                return b_pending.cmp(&a_pending);
            }
        }
        b.updated_at
            .partial_cmp(&a.updated_at)
            .unwrap_or(Ordering::Equal)
    });

    let summaries = sessions.iter().map(to_session_summary).collect();
    Ok(Json(ApiResponse::ok(SessionsData {
        sessions: summaries,
    })))
}

async fn get_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<SessionData>>, ApiError> {
    let (_session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    Ok(Json(ApiResponse::ok(SessionData { session })))
}

async fn delete_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if session.active {
        return Err(ApiError::Conflict(
            "Cannot delete active session. Archive it first.".into(),
        ));
    }

    match state.sync_engine.delete_session(&session_id).await {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => {
            let message = e.to_string();
            if message.contains("active") {
                Err(ApiError::Conflict(message))
            } else {
                Err(ApiError::Internal(message))
            }
        }
    }
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

async fn update_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<RenameBody>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Some(b) => b,
        None => {
            return Err(ApiError::BadRequest(
                "Invalid body: name is required".into(),
            ));
        }
    };

    if body.name.is_empty() || body.name.len() > 255 {
        return Err(ApiError::BadRequest(
            "Invalid body: name is required".into(),
        ));
    }

    let (session_id, _session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    match state
        .sync_engine
        .rename_session(&session_id, &body.name)
        .await
    {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => {
            let message = e.to_string();
            if message.contains("concurrently") || message.contains("version") {
                Err(ApiError::Conflict(message))
            } else {
                Err(ApiError::Internal(message))
            }
        }
    }
}

async fn resume_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ResumeData>>, ApiError> {
    state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    let result = state.sync_engine.resume_session(&id, &auth.namespace).await;
    match result {
        ResumeSessionResult::Success { session_id } => {
            Ok(Json(ApiResponse::ok(ResumeData { session_id })))
        }
        ResumeSessionResult::Error { message, code } => match code {
            ResumeSessionErrorCode::NoMachineOnline => Err(ApiError::ServiceUnavailable(message)),
            ResumeSessionErrorCode::AccessDenied => Err(ApiError::AccessDenied(message)),
            ResumeSessionErrorCode::SessionNotFound => Err(ApiError::NotFound(message)),
            _ => {
                let code_str = match code {
                    ResumeSessionErrorCode::ResumeUnavailable => "resume_unavailable",
                    ResumeSessionErrorCode::ResumeFailed => "resume_failed",
                    _ => "unknown",
                };
                Err(ApiError::Internal(format!("{message} (code: {code_str})")))
            }
        },
    }
}

async fn archive_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    match state.sync_engine.archive_session(&session_id).await {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

async fn abort_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    match state.sync_engine.abort_session(&session_id).await {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

#[derive(Deserialize)]
struct SwitchBody {
    to: Option<String>,
}

async fn switch_session(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<SwitchBody>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    let to = body
        .and_then(|Json(b)| b.to)
        .unwrap_or_else(|| "remote".to_string());

    match state.sync_engine.switch_session(&session_id, &to).await {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

#[derive(Deserialize)]
struct PermissionModeBody {
    mode: PermissionMode,
}

async fn set_permission_mode(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<PermissionModeBody>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Some(b) => b,
        None => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    let flavor = parse_flavor(&session);

    let allowed_modes = permission_modes_for_flavor(flavor);
    if allowed_modes.is_empty() {
        return Err(ApiError::BadRequest(
            "Permission mode not supported for session flavor".into(),
        ));
    }

    if !is_permission_mode_allowed_for_flavor(body.mode, flavor) {
        return Err(ApiError::BadRequest(
            "Invalid permission mode for session flavor".into(),
        ));
    }

    match state
        .sync_engine
        .apply_session_config(&session_id, Some(body.mode), None)
        .await
    {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Conflict(e.to_string())),
    }
}

#[derive(Deserialize)]
struct ModelModeBody {
    model: ModelMode,
}

async fn set_model(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<ModelModeBody>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Some(b) => b,
        None => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    let flavor = parse_flavor(&session);

    if !is_model_mode_allowed_for_flavor(body.model, flavor) {
        return Err(ApiError::BadRequest(
            "Model mode is only supported for Claude sessions".into(),
        ));
    }

    match state
        .sync_engine
        .apply_session_config(&session_id, None, Some(body.model))
        .await
    {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Conflict(e.to_string())),
    }
}

// slash-commands & skills: RPC returns Value, wrap as-is
async fn list_slash_commands(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    let agent = session
        .metadata
        .as_ref()
        .and_then(|m| m.flavor)
        .unwrap_or(AgentFlavor::Claude)
        .as_str()
        .to_string();

    match state
        .sync_engine
        .list_slash_commands(&session_id, &agent)
        .await
    {
        Ok(val) => Ok(Json(ApiResponse::ok(val))),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

async fn list_skills(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    let agent = session
        .metadata
        .as_ref()
        .and_then(|m| m.flavor)
        .unwrap_or(AgentFlavor::Claude)
        .as_str()
        .to_string();

    match state.sync_engine.list_skills(&session_id, &agent).await {
        Ok(val) => Ok(Json(ApiResponse::ok(val))),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

fn estimate_base64_bytes(b64: &str) -> usize {
    let len = b64.len();
    if len == 0 {
        return 0;
    }
    let padding = if b64.ends_with("==") {
        2
    } else if b64.ends_with('=') {
        1
    } else {
        0
    };
    (len * 3 / 4) - padding
}

#[derive(Deserialize)]
struct UploadBody {
    filename: String,
    content: String,
    #[serde(rename = "mimeType")]
    mime_type: String,
}

async fn upload_file(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<UploadBody>>,
) -> Result<Json<ApiResponse<RpcUploadFileResponse>>, ApiError> {
    let Json(body) = match body {
        Some(b) => b,
        None => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    if body.filename.is_empty()
        || body.filename.len() > 255
        || body.content.is_empty()
        || body.mime_type.is_empty()
    {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    if estimate_base64_bytes(&body.content) > MAX_UPLOAD_BYTES {
        return Err(ApiError::PayloadTooLarge(
            "File too large (max 50MB)".into(),
        ));
    }

    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    let resp = state
        .sync_engine
        .upload_file(&session_id, &body.filename, &body.content, &body.mime_type)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(ApiResponse::ok(resp)))
}

#[derive(Deserialize)]
struct UploadDeleteBody {
    path: String,
}

async fn delete_upload_file(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<UploadDeleteBody>>,
) -> Result<Json<ApiResponse<RpcDeleteUploadResponse>>, ApiError> {
    let Json(body) = match body {
        Some(b) => b,
        None => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    if body.path.is_empty() {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    let resp = state
        .sync_engine
        .delete_upload_file(&session_id, &body.path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(ApiResponse::ok(resp)))
}
