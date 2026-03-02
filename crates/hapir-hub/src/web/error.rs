use hapir_shared::frontend::api::ApiError;

use crate::sync::session_cache::SessionAccessError;

impl From<SessionAccessError> for ApiError {
    fn from(e: SessionAccessError) -> Self {
        match e {
            SessionAccessError::NotFound => ApiError::NotFound("Session not found".into()),
            SessionAccessError::AccessDenied => {
                ApiError::AccessDenied("Session access denied".into())
            }
        }
    }
}
