use axum::{
    Json, Router,
    extract::{Extension, State},
    routing::{delete, get, post},
};
use serde::Deserialize;

use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::VapidKeyData;

use crate::store::push_subscriptions::{self, PushSubscriptionInput};
use crate::web::AppState;
use crate::web::middleware::auth::AuthContext;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/push/vapid-public-key", get(vapid_public_key))
        .route("/push/subscribe", post(subscribe))
        .route("/push/subscribe", delete(unsubscribe))
}

async fn vapid_public_key(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<VapidKeyData>>, ApiError> {
    match &state.vapid_public_key {
        Some(key) => Ok(Json(ApiResponse::ok(VapidKeyData {
            public_key: key.clone(),
        }))),
        None => Err(ApiError::NotFound("VAPID public key not configured".into())),
    }
}

#[derive(Deserialize)]
struct PushKeys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
struct SubscribeBody {
    endpoint: String,
    keys: PushKeys,
}

async fn subscribe(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    body: Result<Json<SubscribeBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Ok(b) => b,
        Err(_) => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    if body.endpoint.is_empty() || body.keys.p256dh.is_empty() || body.keys.auth.is_empty() {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    let conn = state.store.conn();
    push_subscriptions::add_push_subscription(
        &conn,
        &auth.namespace,
        &PushSubscriptionInput {
            endpoint: &body.endpoint,
            p256dh: &body.keys.p256dh,
            auth: &body.keys.auth,
        },
    );

    Ok(Json(ApiResponse::success()))
}

#[derive(Deserialize)]
struct UnsubscribeBody {
    endpoint: String,
}

async fn unsubscribe(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    body: Result<Json<UnsubscribeBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Ok(b) => b,
        Err(_) => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    if body.endpoint.is_empty() {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    let conn = state.store.conn();
    push_subscriptions::remove_push_subscription(&conn, &auth.namespace, &body.endpoint);

    Ok(Json(ApiResponse::success()))
}
