//! Frontend-facing DTOs.
//!
//! These are deliberately flatter than bollard's models: the UI gets exactly
//! the fields it renders, in camelCase, so the TypeScript types stay small and
//! the daemon's schema churn doesn't leak into React components.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Docker,
    Podman,
}

impl RuntimeKind {
    /// Stable lowercase name, used in stream keys and log lines.
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeKind::Docker => "docker",
            RuntimeKind::Podman => "podman",
        }
    }
}

/// One entry in the runtime switcher.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub kind: RuntimeKind,
    /// The daemon answered and we can drive it.
    pub available: bool,
    /// Present on this machine at all — CLI binary, socket, or env var.
    ///
    /// Distinct from `available` on purpose. "Installed but not running" is a
    /// fixable state the user should see, with the fix attached; "not installed"
    /// is noise for someone who has never used that runtime, and the UI hides it.
    pub installed: bool,
    /// Server version string, when we could reach the daemon.
    pub version: Option<String>,
    pub api_version: Option<String>,
    /// Why it isn't available, for the UI to show instead of a silent absence.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortBinding {
    pub ip: Option<String>,
    pub private_port: u16,
    pub public_port: Option<u16>,
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    pub id: String,
    /// Leading slash stripped; the first name Docker reports.
    pub name: String,
    pub names: Vec<String>,
    pub image: String,
    pub image_id: String,
    pub command: Option<String>,
    pub created: i64,
    /// running | exited | paused | created | restarting | removing | dead
    pub state: String,
    pub status: String,
    pub ports: Vec<PortBinding>,
    pub labels: std::collections::HashMap<String, String>,
    /// healthy | unhealthy | starting | none
    pub health: String,
    /// Compose project label, when the container belongs to a stack.
    pub compose_project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub repo_digests: Vec<String>,
    pub created: i64,
    pub size: i64,
    pub containers: i64,
    pub dangling: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub attachable: bool,
    pub created: Option<String>,
    /// Names of containers currently attached.
    pub containers: Vec<String>,
    pub subnets: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub created_at: Option<String>,
    pub labels: std::collections::HashMap<String, String>,
    pub scope: Option<String>,
    /// Bytes, when the daemon reports usage data (-1 means unknown).
    pub size: Option<i64>,
}

/// One sample of container resource usage, already reduced to what a chart needs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub id: String,
    pub name: String,
    pub cpu_percent: f64,
    pub memory_usage: u64,
    pub memory_limit: u64,
    pub memory_percent: f64,
    pub network_rx: u64,
    pub network_tx: u64,
    pub block_read: u64,
    pub block_write: u64,
    pub pids: u64,
    /// Milliseconds since epoch, stamped when the sample was produced.
    pub timestamp: i64,
}

/// A single log line pushed to the frontend during follow mode.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub container_id: String,
    /// stdout | stderr
    pub stream: String,
    pub message: String,
    /// Sequence number so the UI can order and de-duplicate.
    pub seq: u64,
}

/// Progress event from `docker pull`, one per layer update.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub image: String,
    pub id: Option<String>,
    pub status: String,
    pub current: Option<i64>,
    pub total: Option<i64>,
    /// 0.0 to 1.0 across all layers, or None before any totals are known.
    pub overall: Option<f64>,
    pub done: bool,
    pub error: Option<String>,
}

/// Progress while copying an image from one runtime to another.
///
/// Docker and Podman keep entirely separate image stores, so "the same image"
/// on both means physically transferring it. We stream the source's export
/// straight into the destination's import — no temp file, no CLI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageTransferProgress {
    pub image: String,
    pub from: RuntimeKind,
    pub to: RuntimeKind,
    /// transferring | importing | complete | failed
    pub phase: String,
    /// Bytes read from the source so far.
    pub bytes: u64,
    /// Source image size, when known, so a percentage can be shown.
    pub total: Option<i64>,
    pub overall: Option<f64>,
    /// Messages emitted by the destination while it loads layers.
    pub status: Option<String>,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeService {
    pub name: String,
    pub project: String,
    pub state: String,
    pub image: String,
    pub container_id: String,
    pub ports: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSummary {
    pub runtime: RuntimeKind,
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub arch: String,
    pub kernel: Option<String>,
    pub storage_driver: Option<String>,
    pub cpus: i64,
    pub memory: i64,
    pub containers_running: i64,
    pub containers_paused: i64,
    pub containers_stopped: i64,
    pub images: i64,
}

/// Options for creating a container from an image.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContainerRequest {
    pub image: String,
    pub name: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    /// "8080:80" or "8080:80/udp"
    #[serde(default)]
    pub ports: Vec<String>,
    /// "/host/path:/container/path" or "volname:/container/path"
    #[serde(default)]
    pub volumes: Vec<String>,
    pub network: Option<String>,
    /// Exact argv, not a shell string.
    ///
    /// Deliberately a vector: whitespace-splitting a command string looks like
    /// a shell but isn't, so `sh -c "while true; do :; done"` would silently
    /// become seven arguments and `-c` would receive only `while`. Callers that
    /// want shell semantics pass them explicitly:
    /// `["sh", "-c", "while true; do :; done"]`.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub auto_remove: bool,
    pub restart_policy: Option<String>,
}

// --------------------------------------------------------------------- activity

/// Whether an operation only observed state or changed it.
///
/// The UI defaults to writes. Reads are dominated by the 4-second list poll and
/// the 1 Hz stats stream, which would bury anything the user actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpKind {
    Read,
    Write,
}

/// One operation Cleat performed against a runtime.
///
/// This exists because Cleat holds root-equivalent control of the daemon.
/// Asserting in a document that it makes only the calls it claims to is weaker
/// than showing them, so every call through `ContainerRuntime` lands here.
///
/// `requests` holds the **actual** Engine API requests — method, path and query
/// as captured off the wire, not reconstructed CLI commands. Cleat speaks the
/// Engine API; there is no `docker run` behind these operations to reveal, and
/// printing one would be showing a translation while implying it was a
/// recording. It is a list because one operation can issue several: opening a
/// shell creates the exec session, starts it, and then sizes the terminal —
/// three round trips, and showing one would be a half-truth. Compose is the one
/// exception and reports its real argv, because compose genuinely is a
/// subprocess.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    /// Monotonic within a session; also the React list key.
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub runtime: RuntimeKind,
    /// Trait method name, e.g. `create_container`.
    pub op: String,
    pub kind: OpKind,
    /// The Engine API requests issued, in order; the literal argv for compose.
    /// Empty when the operation failed before reaching the daemon.
    pub requests: Vec<String>,
    /// Arguments worth seeing, already redacted.
    pub args: Vec<(String, String)>,
    pub duration_ms: u64,
    /// None when the call succeeded.
    pub error: Option<String>,
}
