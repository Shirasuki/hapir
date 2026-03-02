use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    routing::{get, post},
};

use hapir_shared::cli::cli_api::{
    CreateMachineRequest, CreateMachineResponse, CreateSessionRequest, CreateSessionResponse,
    ListMessagesQuery, ListMessagesResponse,
};
use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::MachineData;

use crate::web::AppState;
use crate::web::middleware::cli_auth::CliAuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        // POST /cli/sessions — create or resume a session by tag
        .route("/sessions", post(create_session))
        // GET  /cli/sessions/:id — get session by id or tag
        .route("/sessions/{id}", get(get_session))
        // GET  /cli/sessions/:id/messages — list messages after a given seq
        .route("/sessions/{id}/messages", get(list_messages))
        // POST /cli/machines — register or update a machine
        .route("/machines", post(create_machine))
        // GET  /cli/machines/:id — get machine by id
        .route("/machines/{id}", get(get_machine))
}

async fn create_session(
    State(state): State<AppState>,
    Extension(auth): Extension<CliAuthContext>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<Json<ApiResponse<CreateSessionResponse>>, ApiError> {
    match state
        .sync_engine
        .get_or_create_session(
            &body.tag,
            &body.metadata,
            body.agent_state.as_ref(),
            &auth.namespace,
        )
        .await
    {
        Ok(session) => Ok(Json(ApiResponse::ok(CreateSessionResponse { session }))),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

async fn get_session(
    State(state): State<AppState>,
    Extension(auth): Extension<CliAuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<CreateSessionResponse>>, ApiError> {
    let (_session_id, session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    Ok(Json(ApiResponse::ok(CreateSessionResponse { session })))
}

async fn list_messages(
    State(state): State<AppState>,
    Extension(auth): Extension<CliAuthContext>,
    Path(id): Path<String>,
    Query(query): Query<ListMessagesQuery>,
) -> Result<Json<ApiResponse<ListMessagesResponse>>, ApiError> {
    let (resolved_session_id, _session) = state
        .sync_engine
        .resolve_session_access(&id, &auth.namespace)
        .await?;

    let limit = query.limit.unwrap_or(200).clamp(1, 200);
    let messages =
        state
            .sync_engine
            .get_messages_after(&resolved_session_id, query.after_seq, limit);

    Ok(Json(ApiResponse::ok(ListMessagesResponse { messages })))
}

async fn create_machine(
    State(state): State<AppState>,
    Extension(auth): Extension<CliAuthContext>,
    Json(body): Json<CreateMachineRequest>,
) -> Result<Json<ApiResponse<CreateMachineResponse>>, ApiError> {
    if let Some(existing) = state.sync_engine.get_machine(&body.id).await
        && existing.namespace != auth.namespace
    {
        return Err(ApiError::AccessDenied("Machine access denied".into()));
    }

    let runner_state_value = body
        .runner_state
        .as_ref()
        .and_then(|s| serde_json::to_value(s).ok());

    match state
        .sync_engine
        .get_or_create_machine(
            &body.id,
            &body.metadata,
            runner_state_value.as_ref(),
            &auth.namespace,
        )
        .await
    {
        Ok(machine) => Ok(Json(ApiResponse::ok(CreateMachineResponse {
            machine: serde_json::to_value(machine).unwrap(),
        }))),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

async fn get_machine(
    State(state): State<AppState>,
    Extension(auth): Extension<CliAuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MachineData>>, ApiError> {
    match state
        .sync_engine
        .get_machine_by_namespace(&id, &auth.namespace)
        .await
    {
        Some(machine) => {
            let val = serde_json::to_value(machine).unwrap();
            Ok(Json(ApiResponse::ok(MachineData { machine: val })))
        }
        None => {
            if state.sync_engine.get_machine(&id).await.is_some() {
                Err(ApiError::AccessDenied("Machine access denied".into()))
            } else {
                Err(ApiError::NotFound("Machine not found".into()))
            }
        }
    }
}
