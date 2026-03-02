use axum::http::HeaderValue;
use axum::http::header::AUTHORIZATION;
use axum::{
    Json,
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use subtle::ConstantTimeEq;

use hapir_shared::common::version::PROTOCOL_VERSION;
use hapir_shared::frontend::api::ApiResponse;

use crate::web::AppState;

/// CLI auth context stored in request extensions.
#[derive(Debug, Clone)]
pub struct CliAuthContext {
    pub namespace: String,
}

/// Parse access token format: "baseToken:namespace" or just "baseToken" (default namespace).
fn parse_access_token(raw: &str) -> Option<(String, String)> {
    if raw.is_empty() {
        return None;
    }
    if let Some(pos) = raw.rfind(':') {
        let base = &raw[..pos];
        let ns = &raw[pos + 1..];
        if !base.is_empty() && !ns.is_empty() {
            return Some((base.to_string(), ns.to_string()));
        }
    }
    Some((raw.to_string(), "default".to_string()))
}

macro_rules! reject {
    ($msg:expr) => {
        return Err(Json(ApiResponse::<()>::error(401, $msg)).into_response())
    };
}

/// CLI auth middleware. Validates bearer token with constant-time comparison.
pub async fn cli_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, Response> {
    let auth_header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let auth_header = match auth_header {
        Some(h) => h,
        None => reject!("Missing Authorization header"),
    };

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => reject!("Invalid Authorization header"),
    };

    let (base_token, namespace) = match parse_access_token(token) {
        Some(pair) => pair,
        None => reject!("Invalid token"),
    };

    let expected = state.cli_api_token.as_bytes();
    let provided = base_token.as_bytes();
    if expected.len() != provided.len() || expected.ct_eq(provided).unwrap_u8() != 1 {
        reject!("Invalid token");
    }

    req.extensions_mut().insert(CliAuthContext { namespace });
    let mut response = next.run(req).await;
    response.headers_mut().insert(
        "X-Hapir-Protocol-Version",
        HeaderValue::from_str(&PROTOCOL_VERSION.to_string()).unwrap(),
    );
    Ok(response)
}
