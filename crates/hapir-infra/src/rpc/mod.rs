pub mod types;

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// Build a scoped RPC method name: `"{scope_id}:{method}"`.
pub fn scoped_method(scope_id: &str, method: &str) -> String {
    format!("{scope_id}:{method}")
}

/// A group of related RPC handlers that can be registered together.
///
/// Generic over the registry type so the same group can work with
/// `WsSessionClient`, `WsMachineClient`, or any other `RpcRegistry`.
pub trait RpcHandlerGroup<R: RpcRegistry + ?Sized>: Send + Sync {
    fn register_rpc_handlers<'a>(
        &'a self,
        registry: &'a R,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Trait for registering RPC handlers.
///
/// Implementors scope the method name (e.g. with a session or machine ID)
/// and forward the handler to the underlying WebSocket client.
pub trait RpcRegistry: Send + Sync {
    fn register_rpc<F, Fut>(&self, method: &str, handler: F) -> impl Future<Output = ()> + Send
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Value> + Send + 'static;

    fn register_rpc_group<'a>(
        &'a self,
        group: &'a impl RpcHandlerGroup<Self>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move { group.register_rpc_handlers(self).await }
    }
}

/// A group of related event handlers that can be registered together.
///
/// Parallel to [`RpcHandlerGroup`] but for fire-and-forget event listeners.
pub trait EventHandlerGroup<R: EventRegistry + ?Sized>: Send + Sync {
    fn register_event_handlers<'a>(
        &'a self,
        registry: &'a R,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// Trait for registering event listeners and emitting events.
///
/// Unlike [`RpcRegistry`] (request/response), events are fire-and-forget.
pub trait EventRegistry: Send + Sync {
    fn on(
        &self,
        event: impl Into<String> + Send,
        handler: impl Fn(Value) + Send + Sync + 'static,
    ) -> impl Future<Output = ()> + Send;

    fn emit(&self, event: impl Into<String> + Send, data: Value)
    -> impl Future<Output = ()> + Send;

    fn register_event_group<'a>(
        &'a self,
        group: &'a impl EventHandlerGroup<Self>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move { group.register_event_handlers(self).await }
    }
}
