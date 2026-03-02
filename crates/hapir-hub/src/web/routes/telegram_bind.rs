use axum::{Json, Router, extract::State, routing::post};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Deserialize;

use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::{AuthData, AuthUser};

use crate::config::cli_api_token::{constant_time_eq, parse_access_token};
use crate::config::owner_id::get_or_create_owner_id;
use crate::store::users;
use crate::web::AppState;
use crate::web::middleware::auth::JwtClaims;
use crate::web::telegram_init_data::{TelegramInitDataValidation, validate_telegram_init_data};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindBody {
    init_data: String,
    access_token: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/bind", post(bind_handler))
}

/// Bind a Telegram account to a CLI namespace, enabling Telegram-based login and notifications.
async fn bind_handler(
    State(state): State<AppState>,
    Json(body): Json<BindBody>,
) -> Result<Json<ApiResponse<AuthData>>, ApiError> {
    let bot_token = match &state.telegram_bot_token {
        Some(t) => t.clone(),
        None => {
            return Err(ApiError::ServiceUnavailable(
                "Telegram bot not configured".into(),
            ));
        }
    };

    let validation = validate_telegram_init_data(&body.init_data, &bot_token, 300);
    let tg_user = match validation {
        TelegramInitDataValidation::Ok { user, .. } => user,
        TelegramInitDataValidation::Err(e) => {
            return Err(ApiError::Unauthorized(format!("Invalid init data: {e}")));
        }
    };

    let parsed = match parse_access_token(&body.access_token) {
        Some(p) => p,
        None => {
            return Err(ApiError::Unauthorized("Invalid access token".into()));
        }
    };

    if !constant_time_eq(&parsed.base_token, &state.cli_api_token) {
        return Err(ApiError::Unauthorized("Invalid access token".into()));
    }

    let namespace = parsed.namespace;
    let platform_user_id = tg_user.id.to_string();

    let conn = state.store.conn();
    let existing_user = users::get_user(&conn, "telegram", &platform_user_id);

    if let Some(ref existing) = existing_user {
        if existing.namespace != namespace {
            return Err(ApiError::Conflict("already_bound".into()));
        }
    }

    if existing_user.is_none()
        && let Err(e) = users::add_user(&conn, "telegram", &platform_user_id, &namespace)
    {
        return Err(ApiError::Internal(format!("Failed to create user: {e}")));
    }

    drop(conn);

    let owner_id = match get_or_create_owner_id(&state.data_dir) {
        Ok(id) => id,
        Err(e) => {
            return Err(ApiError::Internal(format!("Failed to get owner ID: {e}")));
        }
    };

    let now = hapir_shared::common::utils::now_secs();

    let claims = JwtClaims {
        uid: owner_id,
        ns: namespace,
        exp: now + 15 * 60,
    };

    let header = Header::new(Algorithm::HS256);
    let key = EncodingKey::from_secret(&state.jwt_secret);

    match encode(&header, &claims, &key) {
        Ok(token) => Ok(Json(ApiResponse::ok(AuthData {
            token,
            user: AuthUser {
                id: owner_id,
                username: tg_user.username,
                first_name: tg_user.first_name,
                last_name: tg_user.last_name,
            },
        }))),
        Err(_) => Err(ApiError::Internal("Failed to create token".into())),
    }
}
