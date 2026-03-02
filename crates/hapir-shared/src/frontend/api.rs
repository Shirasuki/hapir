use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug)]
pub struct ApiResponseError {
    pub code: u16,
    pub message: String,
}

impl fmt::Display for ApiResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "API error ({}): {}", self.code, self.message)
    }
}

impl std::error::Error for ApiResponseError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub code: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 200,
            message: "ok".into(),
            data: Some(data),
        }
    }

    pub fn into_data(self) -> Result<T, ApiResponseError> {
        if self.code != 200 {
            return Err(ApiResponseError {
                code: self.code,
                message: self.message,
            });
        }
        self.data.ok_or_else(|| ApiResponseError {
            code: self.code,
            message: "empty response data".into(),
        })
    }
}

impl ApiResponse<()> {
    pub fn success() -> Self {
        Self {
            code: 200,
            message: "ok".into(),
            data: None,
        }
    }

    pub fn error(code: u16, msg: impl Into<String>) -> Self {
        Self {
            code,
            message: msg.into(),
            data: None,
        }
    }

    pub fn check(self) -> Result<(), ApiResponseError> {
        if self.code != 200 {
            return Err(ApiResponseError {
                code: self.code,
                message: self.message,
            });
        }
        Ok(())
    }
}

pub enum ApiError {
    NotFound(String),
    AccessDenied(String),
    BadRequest(String),
    Conflict(String),
    Unauthorized(String),
    PayloadTooLarge(String),
    ServiceUnavailable(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, message) = match self {
            ApiError::NotFound(msg) => (404, msg),
            ApiError::AccessDenied(msg) => (403, msg),
            ApiError::BadRequest(msg) => (400, msg),
            ApiError::Conflict(msg) => (409, msg),
            ApiError::Unauthorized(msg) => (401, msg),
            ApiError::PayloadTooLarge(msg) => (413, msg),
            ApiError::ServiceUnavailable(msg) => (503, msg),
            ApiError::Internal(msg) => (500, msg),
        };
        (
            StatusCode::OK,
            Json(ApiResponse::<()>::error(code, message)),
        )
            .into_response()
    }
}
