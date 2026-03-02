use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;

use hapir_shared::common::message::AttachmentMetadata;
use hapir_shared::frontend::api::{ApiError, ApiResponse};

use crate::web::AppState;
use crate::web::middleware::auth::AuthContext;
use crate::web::response_types::MessagesData;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions/{id}/messages", get(list_messages))
        .route("/sessions/{id}/messages", post(create_message))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagesQuery {
    limit: Option<i64>,
    before_seq: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageBody {
    text: String,
    local_id: Option<String>,
    attachments: Option<Vec<AttachmentMetadata>>,
}

async fn list_messages(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<ApiResponse<MessagesData>>, ApiError> {
    let (session_id, _session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let before_seq = query.before_seq.filter(|&s| s >= 1);

    let result = state
        .sync_engine
        .get_messages_page(&session_id, limit, before_seq);

    Ok(Json(ApiResponse::ok(result)))
}

async fn create_message(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<SendMessageBody>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Some(b) => b,
        None => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    let has_text = !body.text.is_empty();
    let has_attachments = body.attachments.as_ref().is_some_and(|a| !a.is_empty());

    if !has_text && !has_attachments {
        return Err(ApiError::BadRequest(
            "Message requires text or attachments".into(),
        ));
    }

    let (session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    if !session.active {
        return Err(ApiError::Conflict("Session is inactive".into()));
    }

    let attachments_slice = body.attachments.as_deref();

    match state
        .sync_engine
        .send_message(
            &session_id,
            &auth.namespace,
            &body.text,
            body.local_id.as_deref(),
            attachments_slice,
            Some("webapp"),
        )
        .await
    {
        Ok(()) => Ok(Json(ApiResponse::success())),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}
