use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing::post,
};
use serde::Deserialize;
use serde_json::Value;

use hapir_shared::common::agent_state::{AnswersFormat, PermissionDecision};
use hapir_shared::common::modes::{AgentFlavor, PermissionMode, is_permission_mode_allowed_for_flavor};
use hapir_shared::frontend::api::{ApiError, ApiResponse};

use crate::web::AppState;
use crate::web::middleware::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/sessions/{id}/permissions/{request_id}/approve",
            post(approve_permission),
        )
        .route(
            "/sessions/{id}/permissions/{request_id}/deny",
            post(deny_permission),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveBody {
    mode: Option<PermissionMode>,
    allow_tools: Option<Vec<String>>,
    decision: Option<PermissionDecision>,
    answers: Option<AnswersFormat>,
}

#[derive(Deserialize)]
struct DenyBody {
    decision: Option<PermissionDecision>,
}

async fn approve_permission(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((id, request_id)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let raw = body.map(|Json(v)| v).unwrap_or(serde_json::json!({}));
    let parsed: ApproveBody = match serde_json::from_value(raw) {
        Ok(b) => b,
        Err(_) => {
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

    let request_exists = session
        .agent_state
        .as_ref()
        .and_then(|s| s.requests.as_ref())
        .is_some_and(|r| r.contains_key(&request_id));

    if !request_exists {
        return Err(ApiError::NotFound("Request not found".into()));
    }

    if let Some(mode) = parsed.mode {
        let flavor = session.metadata.as_ref().and_then(|m| m.flavor);

        if !is_permission_mode_allowed_for_flavor(mode, flavor) {
            return Err(ApiError::BadRequest(
                "Invalid permission mode for session flavor".into(),
            ));
        }
    }

    let decision_str = parsed.decision.map(|d| match d {
        PermissionDecision::Approved => "approved",
        PermissionDecision::ApprovedForSession => "approved_for_session",
        PermissionDecision::Denied => "denied",
        PermissionDecision::Abort => "abort",
    });

    match state
        .sync_engine
        .approve_permission(
            &session_id,
            &request_id,
            parsed.mode,
            parsed.allow_tools,
            decision_str,
            parsed.answers,
        )
        .await
    {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

async fn deny_permission(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path((id, request_id)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let raw = body.map(|Json(v)| v).unwrap_or(serde_json::json!({}));
    let parsed: DenyBody = match serde_json::from_value(raw) {
        Ok(b) => b,
        Err(_) => {
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

    let request_exists = session
        .agent_state
        .as_ref()
        .and_then(|s| s.requests.as_ref())
        .is_some_and(|r| r.contains_key(&request_id));

    if !request_exists {
        return Err(ApiError::NotFound("Request not found".into()));
    }

    let decision_str = parsed.decision.map(|d| match d {
        PermissionDecision::Approved => "approved",
        PermissionDecision::ApprovedForSession => "approved_for_session",
        PermissionDecision::Denied => "denied",
        PermissionDecision::Abort => "abort",
    });

    match state
        .sync_engine
        .deny_permission(&session_id, &request_id, decision_str)
        .await
    {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}
