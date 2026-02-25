pub mod auth;
pub mod telegram_bind;
pub mod cli;
pub mod events;
pub mod session_workspace;
pub mod machines;
pub mod messages;
pub mod permissions;
pub mod push;
pub mod sessions;
pub mod voice;

use crate::web::AppState;
use axum::Router;

/// Build the /api router (JWT auth middleware applied externally).
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(auth::router())
        .merge(telegram_bind::router())
        .merge(sessions::router())
        .merge(messages::router())
        .merge(permissions::router())
        .merge(machines::router())
        .merge(events::router())
        .merge(session_workspace::router())
        .merge(push::router())
        .merge(voice::router())
}
