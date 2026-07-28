//! Shared application state.

use crate::error::{AppError, AppResult};
use crate::model::RuntimeKind;
use crate::runtime::{self, ContainerRuntime};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Handle to a running stream task (log follow / stats), so the frontend can
/// stop it when a modal closes instead of leaking a task per open-and-close.
pub struct StreamHandle {
    pub handle: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
pub struct AppState {
    runtime: RwLock<Option<Arc<dyn ContainerRuntime>>>,
    /// Keyed by the event channel name the frontend listens on.
    streams: RwLock<HashMap<String, StreamHandle>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    /// The active runtime, or an error telling the UI to pick one.
    pub async fn runtime(&self) -> AppResult<Arc<dyn ContainerRuntime>> {
        self.runtime
            .read()
            .await
            .clone()
            .ok_or_else(|| AppError::NoRuntime("no runtime selected".into()))
    }

    pub async fn current_kind(&self) -> Option<RuntimeKind> {
        self.runtime.read().await.as_ref().map(|r| r.kind())
    }

    /// Switch runtimes. The new one is connected and pinged *before* the old
    /// one is dropped, so a failed switch leaves the app on a working runtime.
    pub async fn select_runtime(&self, kind: RuntimeKind) -> AppResult<()> {
        let next: Arc<dyn ContainerRuntime> = runtime::connect(kind).await?.into();
        self.stop_all_streams().await;
        *self.runtime.write().await = Some(next);
        Ok(())
    }

    /// Pick the first runtime that answers, so the app is usable on launch
    /// without the user choosing anything. Docker is tried first.
    pub async fn auto_select(&self) -> Option<RuntimeKind> {
        for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
            if self.select_runtime(kind).await.is_ok() {
                return Some(kind);
            }
        }
        None
    }

    pub async fn register_stream(&self, key: String, handle: tokio::task::JoinHandle<()>) {
        // Replacing an existing key means the frontend re-subscribed; abort the
        // previous task rather than letting two writers share one channel.
        if let Some(old) = self
            .streams
            .write()
            .await
            .insert(key, StreamHandle { handle })
        {
            old.handle.abort();
        }
    }

    pub async fn stop_stream(&self, key: &str) -> bool {
        match self.streams.write().await.remove(key) {
            Some(s) => {
                s.handle.abort();
                true
            }
            None => false,
        }
    }

    pub async fn stop_all_streams(&self) {
        let mut streams = self.streams.write().await;
        for (_, s) in streams.drain() {
            s.handle.abort();
        }
    }
}
