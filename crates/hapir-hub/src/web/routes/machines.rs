use std::collections::{HashMap, HashSet};

use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    routing::{get, post},
};
use serde::Deserialize;

use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::{MachinesData, PathsExistsData};

use crate::sync::machine_cache::Machine;
use crate::sync::rpc_gateway::SpawnSessionResult;
use crate::web::AppState;
use crate::web::middleware::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/machines", get(list_machines))
        .route("/machines/{id}/spawn", post(spawn_machine))
        .route("/machines/{id}/paths/exists", post(paths_exists))
}

async fn get_active_machine(
    state: &AppState,
    auth: &AuthContext,
    machine_id: &str,
) -> Result<Machine, ApiError> {
    let machine = state
        .sync_engine
        .get_machine(machine_id)
        .await
        .ok_or_else(|| ApiError::NotFound("Machine not found".into()))?;
    if machine.namespace != auth.namespace {
        return Err(ApiError::AccessDenied("Machine access denied".into()));
    }
    if !machine.active {
        return Err(ApiError::Conflict("Machine is offline".into()));
    }
    Ok(machine)
}

async fn list_machines(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<ApiResponse<MachinesData>>, ApiError> {
    let machines = state
        .sync_engine
        .get_online_machines_by_namespace(&auth.namespace)
        .await;
    let machines_val: Vec<serde_json::Value> = machines
        .iter()
        .filter_map(|m| {
            serde_json::to_value(m)
                .map_err(|e| tracing::error!("Failed to serialize machine: {e}"))
                .ok()
        })
        .collect();
    Ok(Json(ApiResponse::ok(MachinesData {
        machines: machines_val,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnBody {
    directory: String,
    agent: Option<String>,
    model: Option<String>,
    yolo: Option<bool>,
    session_type: Option<String>,
    worktree_name: Option<String>,
}

async fn spawn_machine(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(machine_id): Path<String>,
    Json(body): Json<SpawnBody>,
) -> Result<Json<ApiResponse<SpawnSessionResult>>, ApiError> {
    if body.directory.is_empty() {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    get_active_machine(&state, &auth, &machine_id).await?;

    let agent = body.agent.as_deref().unwrap_or("claude").to_lowercase();
    if !matches!(agent.as_str(), "claude" | "codex" | "gemini" | "opencode") {
        return Err(ApiError::BadRequest("Invalid agent".into()));
    }
    if let Some(ref st) = body.session_type
        && !matches!(st.as_str(), "simple" | "worktree")
    {
        return Err(ApiError::BadRequest("Invalid sessionType".into()));
    }

    let result = state
        .sync_engine
        .spawn_session(
            &machine_id,
            &body.directory,
            &agent,
            body.model.as_deref(),
            body.yolo,
            body.session_type.as_deref(),
            body.worktree_name.as_deref(),
            None,
        )
        .await;

    match result {
        Ok(spawn_result) => Ok(Json(ApiResponse::ok(spawn_result))),
        Err(e) => {
            let message = if e.contains("not registered") {
                "Machine is offline or not connected. Please check that the runner is running on the target machine.".to_string()
            } else if e.contains("timed out") {
                "Request timed out. The machine may be busy or unreachable.".to_string()
            } else if e.contains("connection lost") || e.contains("send failed") {
                "Lost connection to the machine. Please check the runner status.".to_string()
            } else {
                e
            };
            Err(ApiError::ServiceUnavailable(message))
        }
    }
}

#[derive(Deserialize)]
struct PathsExistsBody {
    paths: Vec<String>,
}

async fn paths_exists(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(machine_id): Path<String>,
    body: Result<Json<PathsExistsBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<PathsExistsData>>, ApiError> {
    let body = match body {
        Ok(Json(b)) => b,
        Err(_) => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    if body.paths.len() > 1000 {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    get_active_machine(&state, &auth, &machine_id).await?;

    let mut seen = HashSet::new();
    let unique_paths: Vec<String> = body.paths
        .into_iter()
        .filter_map(|p| {
            let p = p.trim().to_string();
            (!p.is_empty() && seen.insert(p.clone())).then_some(p)
        })
        .collect();

    if unique_paths.is_empty() {
        return Ok(Json(ApiResponse::ok(PathsExistsData {
            exists: HashMap::new(),
        })));
    }

    match state
        .sync_engine
        .check_paths_exist(&machine_id, &unique_paths)
        .await
    {
        Ok(exists) => Ok(Json(ApiResponse::ok(PathsExistsData { exists }))),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}
