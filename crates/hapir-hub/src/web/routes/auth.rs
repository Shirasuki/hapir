use axum::{Json, Router, extract::State, routing::post};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::Value;

use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::{AuthData, AuthUser};

use crate::config::cli_api_token::{constant_time_eq, parse_access_token};
use crate::config::owner_id::get_or_create_owner_id;
use crate::store::users;
use crate::web::AppState;
use crate::web::middleware::auth::JwtClaims;
use crate::web::telegram_init_data::{TelegramInitDataValidation, validate_telegram_init_data};

pub fn router() -> Router<AppState> {
    Router::new().route("/auth", post(auth_handler))
}

/// Authenticate via CLI access token or Telegram initData and issue a short-lived JWT.
async fn auth_handler(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<AuthData>>, ApiError> {
    let init_data = body.get("initData").and_then(|v| v.as_str());
    let access_token = body.get("accessToken").and_then(|v| v.as_str());

    if !init_data.is_some() && !access_token.is_some() {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    let namespace: String;
    let mut username: Option<String> = None;
    let mut first_name: Option<String> = None;
    let mut last_name: Option<String> = None;

    if let Some(token_str) = access_token {
        let parsed = match parse_access_token(token_str) {
            Some(p) => p,
            None => {
                return Err(ApiError::Unauthorized("Invalid access token".into()));
            }
        };

        if !constant_time_eq(&parsed.base_token, &state.cli_api_token) {
            return Err(ApiError::Unauthorized("Invalid access token".into()));
        }

        first_name = Some("Web User".to_string());
        namespace = parsed.namespace;
    } else if let Some(init_data) = init_data {
        let bot_token = match &state.telegram_bot_token {
            Some(t) => t.clone(),
            None => {
                return Err(ApiError::ServiceUnavailable(
                    "Telegram authentication is disabled. Configure TELEGRAM_BOT_TOKEN.".into(),
                ));
            }
        };

        let result = validate_telegram_init_data(init_data, &bot_token, 300);
        let tg_user = match result {
            TelegramInitDataValidation::Ok { user, .. } => user,
            TelegramInitDataValidation::Err(e) => {
                return Err(ApiError::Unauthorized(e));
            }
        };

        let telegram_user_id = tg_user.id.to_string();
        let conn = state.store.conn();
        let stored_user = users::get_user(&conn, "telegram", &telegram_user_id);
        drop(conn);

        let stored_user = match stored_user {
            Some(u) => u,
            None => {
                return Err(ApiError::Unauthorized("not_bound".into()));
            }
        };

        username = tg_user.username;
        first_name = tg_user.first_name;
        last_name = tg_user.last_name;
        namespace = stored_user.namespace;
    } else {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    let now = hapir_shared::common::utils::now_secs();

    let owner_id = match get_or_create_owner_id(&state.data_dir) {
        Ok(id) => id,
        Err(e) => {
            return Err(ApiError::Internal(format!("Failed to get owner ID: {e}")));
        }
    };

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
                username,
                first_name,
                last_name,
            },
        }))),
        Err(_) => Err(ApiError::Internal("Failed to create token".into())),
    }
}
