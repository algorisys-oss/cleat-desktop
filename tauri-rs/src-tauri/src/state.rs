//! Shared application state.

use crate::error::{AppError, AppResult};
use crate::model::RuntimeKind;
use crate::runtime::audit::{ActivityLog, AuditRuntime};
use crate::runtime::{self, ContainerRuntime, ExecStdin};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};

/// Handle to a running stream task (log follow / stats), so the frontend can
/// stop it when a modal closes instead of leaking a task per open-and-close.
pub struct StreamHandle {
    pub handle: tokio::task::JoinHandle<()>,
}

/// The write half of an interactive exec, parked between IPC calls.
///
/// Every other stream in the app is one-way and needs nothing kept alive but a
/// `JoinHandle`. Exec is the exception: keystrokes arrive one command call at a
/// time and each has to reach the same writer, so it lives here instead of
/// inside the task that owns the output.
pub struct ExecSession {
    /// Resize is addressed by exec id, not by our session key.
    pub exec_id: String,
    stdin: Mutex<ExecStdin>,
}

/// Hand-written because the boxed writer is not `Debug`.
impl std::fmt::Debug for ExecSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecSession")
            .field("exec_id", &self.exec_id)
            .finish_non_exhaustive()
    }
}

impl ExecSession {
    /// Write keystrokes to the process.
    ///
    /// Flushing every time is deliberate. Interactive input is latency-bound,
    /// not throughput-bound, and a buffered newline that never reaches the
    /// shell reads to the user as a hung terminal.
    pub async fn write(&self, data: &[u8]) -> AppResult<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(data).await?;
        stdin.flush().await?;
        Ok(())
    }
}

#[derive(Default)]
pub struct AppState {
    runtime: RwLock<Option<Arc<dyn ContainerRuntime>>>,
    /// Keyed by the event channel name the frontend listens on.
    streams: RwLock<HashMap<String, StreamHandle>>,
    /// Interactive exec sessions, keyed by the same channel name as their
    /// output task in `streams`.
    execs: RwLock<HashMap<String, Arc<ExecSession>>>,
    /// Every operation performed, for the activity panel. Outlives runtime
    /// switches on purpose — "what did I just do" spans them.
    activity: Arc<ActivityLog>,
    /// Selected kubeconfig context, when the user has chosen one.
    ///
    /// A name rather than a client: clients are built per call because their
    /// credentials may come from an exec plugin with a short expiry, so a
    /// cached one works until the token quietly dies. See [`crate::k8s::client`].
    /// `None` means "whatever the kubeconfig says is current", which is what
    /// `kubectl` would do.
    kube_context: RwLock<Option<String>>,
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

    /// The chosen kubeconfig context, if any.
    pub async fn kube_context(&self) -> Option<String> {
        self.kube_context.read().await.clone()
    }

    /// Choose a context. Not validated here — [`crate::k8s::probe`] is how the
    /// UI finds out whether a cluster answers, and selecting an unreachable one
    /// has to stay possible so its error can be shown in place.
    pub async fn select_kube_context(&self, context: Option<String>) {
        *self.kube_context.write().await = context;
    }

    /// A client for the selected context.
    pub async fn kube_client(&self) -> AppResult<kube::Client> {
        let context = self.kube_context().await;
        crate::k8s::client(context.as_deref()).await
    }

    /// Switch runtimes. The new one is connected and pinged *before* the old
    /// one is dropped, so a failed switch leaves the app on a working runtime.
    ///
    /// This is the only place a runtime is constructed, which is why the audit
    /// wrapper goes on here: there is no path to a bare runtime that could
    /// bypass the activity log.
    pub async fn select_runtime(&self, kind: RuntimeKind) -> AppResult<()> {
        let inner = runtime::connect(kind).await?;
        let next: Arc<dyn ContainerRuntime> =
            Arc::new(AuditRuntime::new(inner, self.activity.clone()));
        self.stop_all_streams().await;
        *self.runtime.write().await = Some(next);
        Ok(())
    }

    pub fn activity(&self) -> &Arc<ActivityLog> {
        &self.activity
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
        // Exec sessions are registered in two places; dropping only the output
        // task would strand its stdin writer here, holding an upgraded socket
        // open against a runtime we are about to stop using.
        self.execs.write().await.clear();
    }

    // ------------------------------------------------------------------ exec

    pub async fn register_exec(&self, key: String, exec_id: String, stdin: ExecStdin) {
        let session = Arc::new(ExecSession {
            exec_id,
            stdin: Mutex::new(stdin),
        });
        self.execs.write().await.insert(key, session);
    }

    /// Look up a live session. A miss means the terminal was closed, the
    /// process exited, or the runtime was switched underneath it.
    pub async fn exec(&self, key: &str) -> AppResult<Arc<ExecSession>> {
        self.execs
            .read()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| AppError::NotFound(format!("no exec session '{key}'")))
    }

    /// Drop the session and abort the task pumping its output.
    pub async fn stop_exec(&self, key: &str) -> bool {
        let had_session = self.execs.write().await.remove(key).is_some();
        let had_stream = self.stop_stream(&format!("exec:{key}")).await;
        had_session || had_stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A session whose stdin is one end of a duplex pipe, so the test can read
    /// back exactly what a caller wrote.
    async fn session_with_pipe(state: &AppState, key: &str) -> tokio::io::DuplexStream {
        let (ours, theirs) = tokio::io::duplex(1024);
        state
            .register_exec(key.to_string(), format!("exec-{key}"), Box::pin(theirs))
            .await;
        ours
    }

    #[tokio::test]
    async fn write_reaches_the_process_verbatim() {
        let state = AppState::new();
        let mut pipe = session_with_pipe(&state, "chan-1").await;

        // Includes a multi-byte character and a control byte: nothing in the
        // path may re-encode or line-buffer this.
        let payload = "echo café\r\x03".as_bytes();
        state
            .exec("chan-1")
            .await
            .unwrap()
            .write(payload)
            .await
            .unwrap();

        let mut buf = vec![0u8; payload.len()];
        pipe.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, payload);
    }

    #[tokio::test]
    async fn unknown_session_is_not_found() {
        let state = AppState::new();
        let err = state.exec("no-such-channel").await.unwrap_err();
        assert_eq!(err.kind(), "not_found");
    }

    #[tokio::test]
    async fn stop_exec_drops_the_session() {
        let state = AppState::new();
        let _pipe = session_with_pipe(&state, "chan-2").await;

        assert!(state.stop_exec("chan-2").await, "first stop should find it");
        assert!(
            state.exec("chan-2").await.is_err(),
            "session should be gone"
        );
        assert!(!state.stop_exec("chan-2").await, "second stop is a no-op");
    }

    /// Switching runtimes calls `stop_all_streams`. Before exec, that only had
    /// to abort tasks; now it must also drop stdin writers, or each switch
    /// strands an upgraded socket against the runtime being abandoned.
    #[tokio::test]
    async fn stop_all_streams_clears_exec_sessions() {
        let state = AppState::new();
        let _a = session_with_pipe(&state, "chan-3").await;
        let _b = session_with_pipe(&state, "chan-4").await;

        state.stop_all_streams().await;

        assert!(state.exec("chan-3").await.is_err());
        assert!(state.exec("chan-4").await.is_err());
    }
}
