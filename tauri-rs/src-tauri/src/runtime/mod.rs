//! Runtime abstraction.
//!
//! Every container operation the app performs goes through [`ContainerRuntime`].
//! Docker and Podman both speak the Docker Engine API, so they share the
//! [`engine`] client; what differs (socket discovery, compose binary, feature
//! support) lives in the per-runtime impls. A future nerdctl/containerd backend
//! implements this trait without touching a single command handler.

pub mod audit;
pub mod compose;
pub mod credentials;
pub mod docker;
pub mod engine;
pub mod podman;
pub mod raw;
pub mod wire;

use crate::error::AppResult;
use crate::model::{
    ComposeService, Container, CreateContainerRequest, Image, LogLine, Network, PullProgress,
    RuntimeInfo, RuntimeKind, Stats, SystemSummary, Volume,
};
use async_trait::async_trait;
use futures_util::Stream;
use std::collections::HashMap;
use std::pin::Pin;

/// Streams handed back to the command layer, which forwards them onto Tauri events.
pub type LogStream = Pin<Box<dyn Stream<Item = AppResult<LogLine>> + Send>>;
pub type StatsStream = Pin<Box<dyn Stream<Item = AppResult<Stats>> + Send>>;
pub type PullStream = Pin<Box<dyn Stream<Item = AppResult<PullProgress>> + Send>>;
/// Raw image tarball, streamed between runtimes without buffering to disk.
pub type ByteStream = Pin<Box<dyn Stream<Item = AppResult<bytes::Bytes>> + Send + 'static>>;
/// Status lines emitted by a runtime while it loads an imported image.
pub type ImportStream = Pin<Box<dyn Stream<Item = AppResult<String>> + Send>>;
/// Write half of an attached exec session — keystrokes on their way to the
/// process inside the container.
pub type ExecStdin = Pin<Box<dyn tokio::io::AsyncWrite + Send>>;

/// A live interactive exec session.
///
/// Unlike every other stream in this app, exec is bidirectional: `output` is
/// pumped onto a Tauri event channel, while `stdin` has to outlive the call
/// that created it so later keystrokes can be written to it. The command layer
/// parks it in [`AppState`](crate::state::AppState) for exactly that reason.
///
/// `output` carries *raw* terminal bytes. Because the session is TTY-mode the
/// daemon does not multiplex it, so there are no stream headers to strip — and
/// nothing here may reinterpret the bytes: escape sequences, partial UTF-8 at a
/// chunk boundary and absent trailing newlines are all normal and all
/// significant.
pub struct ExecAttach {
    /// Needed for resize, which is addressed by exec id rather than session.
    pub exec_id: String,
    pub output: ByteStream,
    pub stdin: ExecStdin,
}

/// Hand-written because neither half is `Debug`; `expect_err` in the tests
/// needs the bound.
impl std::fmt::Debug for ExecAttach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecAttach")
            .field("exec_id", &self.exec_id)
            .finish_non_exhaustive()
    }
}

/// The compose verbs the UI can trigger. A closed set, never a free string, so
/// nothing user-supplied can become an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeAction {
    Up,
    Down,
    Restart,
    Stop,
    Start,
}

impl ComposeAction {
    pub fn argv(self) -> Vec<String> {
        match self {
            ComposeAction::Up => vec!["up".into(), "-d".into()],
            ComposeAction::Down => vec!["down".into()],
            ComposeAction::Restart => vec!["restart".into()],
            ComposeAction::Stop => vec!["stop".into()],
            ComposeAction::Start => vec!["start".into()],
        }
    }

    /// `down` tears down the whole project; naming a service is meaningless.
    pub fn accepts_service(self) -> bool {
        !matches!(self, ComposeAction::Down)
    }
}

#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    fn kind(&self) -> RuntimeKind;

    // -- system ------------------------------------------------------------
    async fn system_summary(&self) -> AppResult<SystemSummary>;

    // -- containers --------------------------------------------------------
    async fn list_containers(&self, all: bool) -> AppResult<Vec<Container>>;
    async fn inspect_container(&self, id: &str) -> AppResult<serde_json::Value>;
    async fn start_container(&self, id: &str) -> AppResult<()>;
    async fn stop_container(&self, id: &str) -> AppResult<()>;
    async fn restart_container(&self, id: &str) -> AppResult<()>;
    async fn pause_container(&self, id: &str) -> AppResult<()>;
    async fn unpause_container(&self, id: &str) -> AppResult<()>;
    async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> AppResult<()>;
    async fn create_container(&self, req: CreateContainerRequest) -> AppResult<String>;
    async fn container_health(&self, id: &str) -> AppResult<String>;

    /// One-shot log fetch (`tail` lines, no follow).
    async fn container_logs(&self, id: &str, tail: i64, timestamps: bool) -> AppResult<String>;
    /// Follow mode. The stream ends when the container stops or the task is dropped.
    async fn follow_logs(&self, id: &str, tail: i64) -> AppResult<LogStream>;

    /// Single stats sample.
    async fn container_stats(&self, id: &str) -> AppResult<Stats>;
    /// Continuous stats, one sample per daemon tick (~1s).
    async fn stream_stats(&self, id: &str) -> AppResult<StatsStream>;

    /// Attach an interactive TTY to a new process inside a running container.
    ///
    /// `argv` is deliberately unconstrained — running a command of the user's
    /// choosing *is* the feature, and it is safe here for the reason the
    /// compose path is not: this is an argv vector handed to the daemon, never
    /// a shell string. See `docs/security.md`.
    async fn exec_start(
        &self,
        id: &str,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> AppResult<ExecAttach>;

    /// Tell the daemon the terminal's new size. Without this the process sees
    /// whatever it was given at attach time, and full-screen programs draw into
    /// the wrong geometry.
    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> AppResult<()>;

    /// Best-effort probe for an interactive shell that exists in this image.
    async fn detect_shell(&self, id: &str) -> AppResult<String>;

    /// List services inside a container via its init system.
    async fn list_container_services(&self, id: &str) -> AppResult<Vec<(String, String)>>;
    /// `start` | `stop` | `restart` a service inside a container.
    async fn control_container_service(
        &self,
        id: &str,
        service: &str,
        action: &str,
    ) -> AppResult<String>;

    // -- images ------------------------------------------------------------
    async fn list_images(&self) -> AppResult<Vec<Image>>;
    async fn pull_image(&self, image: &str) -> AppResult<PullStream>;
    /// Point a second reference at an existing image, so it can be pushed
    /// somewhere its current name does not allow.
    async fn tag_image(&self, source: &str, target: &str) -> AppResult<()>;
    /// Push a reference to the registry its name points at, authenticating from
    /// the runtime's existing login. Reports the same progress shape as a pull.
    async fn push_image(&self, reference: &str) -> AppResult<PullStream>;
    async fn remove_image(&self, id: &str, force: bool) -> AppResult<()>;
    async fn image_history(&self, id: &str) -> AppResult<serde_json::Value>;
    async fn inspect_image(&self, id: &str) -> AppResult<serde_json::Value>;
    async fn prune_images(&self) -> AppResult<u64>;

    /// Stream an image out as an uncompressed TAR archive.
    async fn export_image(&self, reference: &str) -> AppResult<ByteStream>;
    /// Load an image from a TAR stream, reporting the runtime's status lines.
    async fn import_image(&self, tar: ByteStream) -> AppResult<ImportStream>;

    // -- networks ----------------------------------------------------------
    async fn list_networks(&self) -> AppResult<Vec<Network>>;
    async fn create_network(&self, name: &str, driver: &str, internal: bool) -> AppResult<String>;
    async fn remove_network(&self, id: &str) -> AppResult<()>;
    async fn inspect_network(&self, id: &str) -> AppResult<serde_json::Value>;
    async fn connect_network(&self, network: &str, container: &str) -> AppResult<()>;
    async fn disconnect_network(&self, network: &str, container: &str) -> AppResult<()>;

    // -- volumes -----------------------------------------------------------
    async fn list_volumes(&self) -> AppResult<Vec<Volume>>;
    /// Bytes on disk per volume name.
    ///
    /// Separate from `list_volumes` because the volume listing never carries
    /// usage: only the daemon's disk-usage endpoint computes it, and computing
    /// it means walking every volume, which takes seconds. The listing stays
    /// cheap enough to poll; this is asked for once.
    async fn volume_usage(&self) -> AppResult<HashMap<String, i64>>;
    async fn create_volume(&self, name: &str, driver: Option<&str>) -> AppResult<Volume>;
    async fn remove_volume(&self, name: &str, force: bool) -> AppResult<()>;
    async fn inspect_volume(&self, name: &str) -> AppResult<serde_json::Value>;
    async fn prune_volumes(&self) -> AppResult<u64>;

    // -- compose -----------------------------------------------------------
    /// The compose CLI for this runtime, as argv (e.g. `["docker", "compose"]`).
    fn compose_argv(&self) -> Vec<String>;
    async fn compose_services(&self, project_path: &str) -> AppResult<Vec<ComposeService>>;
    async fn compose_up(&self, project_path: &str) -> AppResult<String>;
    async fn compose_down(&self, project_path: &str) -> AppResult<String>;
    async fn compose_restart(&self, project_path: &str, service: Option<&str>) -> AppResult<String>;
    /// Run `up`/`down`/`restart` with live output.
    ///
    /// Preferred over the blocking variants above for anything that starts or
    /// stops containers: Docker's 10-second SIGTERM grace period per stubborn
    /// container makes those calls slow enough that a spinner with no output
    /// reads as a hang.
    async fn compose_exec(
        &self,
        project_path: &str,
        action: ComposeAction,
        service: Option<&str>,
    ) -> AppResult<compose::LineStream>;
}

/// Probe every known runtime so the UI can populate its switcher.
///
/// Each probe actually pings the daemon; a runtime whose binary exists but
/// whose socket is dead is reported unavailable with the reason attached,
/// rather than appearing selectable and then failing on first use.
///
/// Runtimes that aren't installed at all are still returned, flagged
/// `installed: false`. Filtering is the UI's decision, not this layer's — the
/// backend's job is to report what it found, and a caller that wants the full
/// picture (diagnostics, a "show all" toggle) shouldn't have to re-probe.
pub async fn detect_runtimes() -> Vec<RuntimeInfo> {
    let (docker, podman) = tokio::join!(docker::probe(), podman::probe());
    vec![docker, podman]
}

/// Is `binary` on PATH?
///
/// Used only to tell "not installed" from "installed but not running"; it is
/// never a substitute for actually reaching the daemon.
pub(crate) fn binary_on_path(binary: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(binary);
        candidate.is_file()
    })
}

/// Build a runtime handle for `kind`, failing if its daemon isn't reachable.
pub async fn connect(kind: RuntimeKind) -> AppResult<Box<dyn ContainerRuntime>> {
    match kind {
        RuntimeKind::Docker => Ok(Box::new(docker::DockerRuntime::connect().await?)),
        RuntimeKind::Podman => Ok(Box::new(podman::PodmanRuntime::connect().await?)),
    }
}
