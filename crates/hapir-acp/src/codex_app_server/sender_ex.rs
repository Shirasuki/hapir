use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::mpsc;

/// Wraps an `Arc<StdMutex<Option<UnboundedSender<T>>>>` for convenient forwarding.
/// Clones cheaply and the `send()` method is lock-free in practice (nanosecond hold).
#[derive(Clone)]
pub struct ArcMutexSender<T>(Arc<StdMutex<Option<mpsc::UnboundedSender<T>>>>);

impl<T> ArcMutexSender<T> {
    pub fn new() -> Self {
        Self(Arc::new(StdMutex::new(None)))
    }

    pub fn send(&self, value: T) {
        if let Ok(guard) = self.0.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(value);
            }
        }
    }

    pub fn set(&self, tx: Option<mpsc::UnboundedSender<T>>) {
        *self.0.lock().unwrap() = tx;
    }
}
