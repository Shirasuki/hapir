use std::convert::Infallible;

use axum::{
    extract::{Extension, Query, State}, response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json,
    Router,
};
use futures::stream::Stream;
use serde::Deserialize;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;

use hapir_shared::common::sync_event::{ConnectionChangedData, HeartbeatData, SyncEvent};
use hapir_shared::common::utils::now_millis;
use hapir_shared::frontend::api::{ApiError, ApiResponse};

use crate::sync::sse_manager::SseMessage;
use crate::sync::visibility_tracker::VisibilityState;
use crate::web::middleware::auth::AuthContext;
use crate::web::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/events", get(events_handler))
        .route("/visibility", post(set_visibility))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsQuery {
    all: Option<String>,
    session_id: Option<String>,
    machine_id: Option<String>,
    visibility: Option<String>,
}

async fn events_handler(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let all = matches!(query.all.as_deref(), Some("true") | Some("1"));
    let session_id = query.session_id.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let machine_id = query.machine_id.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let visibility = query.visibility.as_deref()
        .map(VisibilityState::from)
        .unwrap_or(VisibilityState::Hidden);
    let subscription_id = uuid::Uuid::new_v4().to_string();
    let namespace = auth.namespace.clone();

    let resolved_session_id = if let Some(ref sid) = session_id {
        let (resolved_id, _session) = state
            .sync_engine
            .resolve_session_access(sid, &namespace)
            .await?;
        Some(resolved_id)
    } else {
        None
    };

    if let Some(ref mid) = machine_id {
        let machine = state.sync_engine.get_machine(mid).await;
        match machine {
            None => {
                return Err(ApiError::NotFound("Machine not found".into()));
            }
            Some(m) if m.namespace != namespace => {
                return Err(ApiError::AccessDenied("Machine access denied".into()));
            }
            _ => {}
        }
    }

    let (rx, _) = state.sync_engine.subscribe_sse(
        subscription_id.clone(),
        namespace.clone(),
        all,
        resolved_session_id,
        machine_id,
        visibility,
    );

    let sub_id_for_cleanup = subscription_id.clone();
    let state_for_cleanup = state.clone();

    let initial_event = Event::default().data(
        serde_json::to_string(&SyncEvent::ConnectionChanged {
            namespace: None,
            data: Some(ConnectionChangedData {
                status: "connected".to_string(),
                subscription_id: Some(subscription_id),
            }),
        })
        .unwrap_or_default(),
    );

    let receiver_stream = UnboundedReceiverStream::new(rx);

    let event_stream = futures::stream::once(async move { Ok(initial_event) }).chain(
        receiver_stream.map(move |msg| {
            let sync_event = match msg {
                SseMessage::Event(e) => e,
                SseMessage::Heartbeat => SyncEvent::Heartbeat {
                    namespace: Some(namespace.clone()),
                    data: Some(HeartbeatData {
                        timestamp: now_millis() as f64,
                    }),
                },
            };
            let data = serde_json::to_string(&sync_event).unwrap_or_else(|_| "{}".to_string());
            Ok(Event::default().data(data))
        }),
    );

    let cleanup_stream = CleanupStream {
        inner: Box::pin(event_stream),
        state: Some(state_for_cleanup),
        subscription_id: Some(sub_id_for_cleanup),
    };

    Ok(Sse::new(cleanup_stream).keep_alive(KeepAlive::default()))
}

/// A stream wrapper that unsubscribes from the SSE manager when dropped.
struct CleanupStream<S> {
    inner: std::pin::Pin<Box<S>>,
    state: Option<AppState>,
    subscription_id: Option<String>,
}

impl<S> Drop for CleanupStream<S> {
    fn drop(&mut self) {
        if let (Some(state), Some(sub_id)) = (self.state.take(), self.subscription_id.take()) {
            state.sync_engine.unsubscribe_sse(&sub_id);
        }
    }
}

impl<S> Stream for CleanupStream<S>
where
    S: Stream,
{
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisibilityBody {
    subscription_id: String,
    visibility: String,
}

async fn set_visibility(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    body: Result<Json<VisibilityBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(body) = match body {
        Ok(b) => b,
        Err(_) => {
            return Err(ApiError::BadRequest("Invalid body".into()));
        }
    };

    if body.subscription_id.is_empty() {
        return Err(ApiError::BadRequest("Invalid body".into()));
    }

    let vis = VisibilityState::from(body.visibility.as_str());

    let updated = state
        .sync_engine
        .set_sse_visibility(&body.subscription_id, &auth.namespace, vis);

    if !updated {
        return Err(ApiError::NotFound("Subscription not found".into()));
    }

    Ok(Json(ApiResponse::success()))
}
