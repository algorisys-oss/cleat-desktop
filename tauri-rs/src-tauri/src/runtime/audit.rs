//! Records every runtime operation so the user can see what Cleat actually did.
//!
//! # Why a decorator
//!
//! The obvious implementation is to log inside each of the 55 command handlers.
//! That drifts: a new command gets added, nobody adds the logging line, and the
//! panel quietly under-reports — which is worse than not having it, because the
//! whole value is being able to trust that what you see is everything.
//!
//! Everything already goes through `dyn ContainerRuntime`, so wrapping *that*
//! leaves nowhere to bypass. `AppState::select_runtime` is the single place a
//! runtime is constructed, and it wraps unconditionally.
//!
//! # What `requests` is, and what it is not
//!
//! Cleat talks to the Engine API, not the `docker` CLI. There is no shell
//! command behind `start_container` to reveal — it is `POST
//! /containers/{id}/start` over a unix socket. `requests` reports that.
//!
//! Nothing here is authored. The lines are captured off the wire as the request
//! is built ([`crate::runtime::wire`]), so they cannot drift from what was
//! actually sent, and an operation that issues three requests shows three —
//! `exec_start` does, which the string it replaced got wrong. Compose
//! is the exception: [`record_argv`](AuditRuntime::record_argv) takes the
//! executed argv, because compose is a real subprocess and there the command is
//! the truth. It still captures alongside, so a compose path that somehow
//! reached the API would show it rather than hide it.

use crate::error::{AppError, AppResult};
use crate::model::{
    ActivityEntry, ComposeService, Container, CreateContainerRequest, Image, Network, OpKind,
    RuntimeKind, Stats, SystemSummary, Volume,
};
use crate::runtime::{
    compose, wire, ByteStream, ComposeAction, ContainerRuntime, ExecAttach, ImportStream,
    LogStream, PullStream, StatsStream,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How many entries to keep. A long session with the stats stream running can
/// generate thousands; this is a debugging aid, not an audit trail that has to
/// survive.
const CAPACITY: usize = 500;

/// Substrings that mark an environment key as secret.
///
/// Redaction is on by default and matches loosely on purpose: a missed secret
/// ends up on screen and in whatever the user pastes into a bug report, while
/// an over-redacted value costs nothing.
const SECRET_HINTS: [&str; 8] = [
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "credential",
    "auth",
];

/// Mask the value of `KEY=VALUE` when the key looks sensitive.
pub(crate) fn redact_env(entry: &str) -> String {
    let Some((key, value)) = entry.split_once('=') else {
        return entry.to_string();
    };
    let lowered = key.to_ascii_lowercase();
    // `KEY` alone is too common a suffix to match loosely (`SSH_KEY` yes,
    // `SORT_KEY` no), so require it to be the whole key or a suffix.
    let secret = SECRET_HINTS.iter().any(|h| lowered.contains(h))
        || lowered == "key"
        || lowered.ends_with("_key");
    if secret && !value.is_empty() {
        format!("{key}=••••••••")
    } else {
        entry.to_string()
    }
}

#[derive(Default)]
pub struct ActivityLog {
    entries: Mutex<VecDeque<ActivityEntry>>,
    seq: AtomicU64,
}

impl ActivityLog {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, entry: ActivityEntry) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= CAPACITY {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Newest first, which is the order the panel renders.
    pub fn snapshot(&self) -> Vec<ActivityEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.iter().rev().cloned().collect()
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

pub struct AuditRuntime {
    inner: Box<dyn ContainerRuntime>,
    log: Arc<ActivityLog>,
}

impl AuditRuntime {
    pub fn new(inner: Box<dyn ContainerRuntime>, log: Arc<ActivityLog>) -> Self {
        Self { inner, log }
    }

    /// Run `f`, timing it and recording the outcome and the requests it issued.
    ///
    /// Takes the future rather than a closure so each trait method stays a
    /// single readable line, and so there is no way to record an operation
    /// without actually performing it.
    ///
    /// There is deliberately no parameter for the request line: it is captured,
    /// not described, and an authored one could disagree with the wire.
    async fn record<T, F>(
        &self,
        op: &str,
        kind: OpKind,
        args: Vec<(String, String)>,
        f: F,
    ) -> AppResult<T>
    where
        F: Future<Output = AppResult<T>>,
    {
        let started = Instant::now();
        let (result, requests) = wire::capture(f).await;
        self.push(op, kind, requests, args, started, &result);
        result
    }

    /// As [`record`](Self::record), for the compose subprocess.
    ///
    /// `argv` is what was executed, so it leads. Capture still runs: if a
    /// compose path ever issues an API request, it belongs on screen rather
    /// than in the gap between the two mechanisms.
    async fn record_argv<T, F>(
        &self,
        op: &str,
        kind: OpKind,
        argv: String,
        args: Vec<(String, String)>,
        f: F,
    ) -> AppResult<T>
    where
        F: Future<Output = AppResult<T>>,
    {
        let started = Instant::now();
        let (result, mut requests) = wire::capture(f).await;
        requests.insert(0, argv);
        self.push(op, kind, requests, args, started, &result);
        result
    }

    fn push<T>(
        &self,
        op: &str,
        kind: OpKind,
        requests: Vec<String>,
        args: Vec<(String, String)>,
        started: Instant,
        result: &AppResult<T>,
    ) {
        self.log.push(ActivityEntry {
            seq: self.log.seq.fetch_add(1, Ordering::Relaxed),
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            runtime: self.inner.kind(),
            op: op.to_string(),
            kind,
            requests,
            args,
            duration_ms: started.elapsed().as_millis() as u64,
            error: result.as_ref().err().map(AppError::message),
        });
    }
}

/// `vec![]` with less ceremony at the call sites below.
fn args<const N: usize>(pairs: [(&str, String); N]) -> Vec<(String, String)> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

use OpKind::{Read, Write};

#[async_trait]
impl ContainerRuntime for AuditRuntime {
    fn kind(&self) -> RuntimeKind {
        self.inner.kind()
    }

    fn compose_argv(&self) -> Vec<String> {
        self.inner.compose_argv()
    }

    // -- system ------------------------------------------------------------

    async fn system_summary(&self) -> AppResult<SystemSummary> {
        self.record("system_summary", Read, vec![], self.inner.system_summary())
            .await
    }

    // -- containers --------------------------------------------------------

    async fn list_containers(&self, all: bool) -> AppResult<Vec<Container>> {
        self.record(
            "list_containers",
            Read,
            vec![],
            self.inner.list_containers(all),
        )
        .await
    }

    async fn inspect_container(&self, id: &str) -> AppResult<serde_json::Value> {
        self.record(
            "inspect_container",
            Read,
            vec![],
            self.inner.inspect_container(id),
        )
        .await
    }

    async fn start_container(&self, id: &str) -> AppResult<()> {
        self.record(
            "start_container",
            Write,
            vec![],
            self.inner.start_container(id),
        )
        .await
    }

    async fn stop_container(&self, id: &str) -> AppResult<()> {
        self.record(
            "stop_container",
            Write,
            vec![],
            self.inner.stop_container(id),
        )
        .await
    }

    async fn restart_container(&self, id: &str) -> AppResult<()> {
        self.record(
            "restart_container",
            Write,
            vec![],
            self.inner.restart_container(id),
        )
        .await
    }

    async fn pause_container(&self, id: &str) -> AppResult<()> {
        self.record(
            "pause_container",
            Write,
            vec![],
            self.inner.pause_container(id),
        )
        .await
    }

    async fn unpause_container(&self, id: &str) -> AppResult<()> {
        self.record(
            "unpause_container",
            Write,
            vec![],
            self.inner.unpause_container(id),
        )
        .await
    }

    async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> AppResult<()> {
        self.record(
            "remove_container",
            Write,
            vec![],
            self.inner.remove_container(id, force, volumes),
        )
        .await
    }

    async fn create_container(&self, req: CreateContainerRequest) -> AppResult<String> {
        // The most detailed entry in the log, because it is the operation with
        // the most ways to be surprising after the fact.
        let mut shown = args([
            ("image", req.image.clone()),
            ("name", req.name.clone().unwrap_or_else(|| "(auto)".into())),
        ]);
        if !req.ports.is_empty() {
            shown.push(("ports".into(), req.ports.join(", ")));
        }
        if !req.env.is_empty() {
            shown.push((
                "env".into(),
                req.env
                    .iter()
                    .map(|e| redact_env(e))
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        if !req.volumes.is_empty() {
            shown.push(("volumes".into(), req.volumes.join(", ")));
        }
        if let Some(net) = &req.network {
            shown.push(("network".into(), net.clone()));
        }
        if !req.command.is_empty() {
            shown.push(("command".into(), format!("{:?}", req.command)));
        }
        if req.auto_remove {
            shown.push(("autoRemove".into(), "true".into()));
        }
        if let Some(p) = &req.restart_policy {
            shown.push(("restartPolicy".into(), p.clone()));
        }

        self.record(
            "create_container",
            Write,
            shown,
            self.inner.create_container(req),
        )
        .await
    }

    async fn container_health(&self, id: &str) -> AppResult<String> {
        self.record(
            "container_health",
            Read,
            vec![],
            self.inner.container_health(id),
        )
        .await
    }

    async fn container_logs(&self, id: &str, tail: i64, timestamps: bool) -> AppResult<String> {
        self.record(
            "container_logs",
            Read,
            vec![],
            self.inner.container_logs(id, tail, timestamps),
        )
        .await
    }

    async fn follow_logs(&self, id: &str, tail: i64) -> AppResult<LogStream> {
        // Records the subscription, not each line — the stream outlives this
        // call and a per-line entry would drown everything else.
        self.record(
            "follow_logs",
            Read,
            vec![],
            self.inner.follow_logs(id, tail),
        )
        .await
    }

    async fn container_stats(&self, id: &str) -> AppResult<Stats> {
        self.record(
            "container_stats",
            Read,
            vec![],
            self.inner.container_stats(id),
        )
        .await
    }

    async fn stream_stats(&self, id: &str) -> AppResult<StatsStream> {
        self.record("stream_stats", Read, vec![], self.inner.stream_stats(id))
            .await
    }

    async fn exec_start(
        &self,
        id: &str,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> AppResult<ExecAttach> {
        let shown = args([
            ("argv", format!("{argv:?}")),
            ("size", format!("{cols}x{rows}")),
        ]);
        self.record(
            "exec_start",
            Write,
            shown,
            self.inner.exec_start(id, argv, cols, rows),
        )
        .await
    }

    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        // Deliberately a read: resizing is a window-drag artefact, and at write
        // level it would bury the operations the user actually chose.
        self.record(
            "exec_resize",
            Read,
            vec![],
            self.inner.exec_resize(exec_id, cols, rows),
        )
        .await
    }

    async fn detect_shell(&self, id: &str) -> AppResult<String> {
        self.record("detect_shell", Read, vec![], self.inner.detect_shell(id))
            .await
    }

    async fn list_container_services(&self, id: &str) -> AppResult<Vec<(String, String)>> {
        self.record(
            "list_container_services",
            Read,
            vec![],
            self.inner.list_container_services(id),
        )
        .await
    }

    async fn control_container_service(
        &self,
        id: &str,
        service: &str,
        action: &str,
    ) -> AppResult<String> {
        let shown = args([
            ("service", service.to_string()),
            ("action", action.to_string()),
        ]);
        self.record(
            "control_container_service",
            Write,
            shown,
            self.inner.control_container_service(id, service, action),
        )
        .await
    }

    // -- images ------------------------------------------------------------

    async fn list_images(&self) -> AppResult<Vec<Image>> {
        self.record("list_images", Read, vec![], self.inner.list_images())
            .await
    }

    async fn pull_image(&self, image: &str) -> AppResult<PullStream> {
        self.record("pull_image", Write, vec![], self.inner.pull_image(image))
            .await
    }

    async fn remove_image(&self, id: &str, force: bool) -> AppResult<()> {
        self.record(
            "remove_image",
            Write,
            vec![],
            self.inner.remove_image(id, force),
        )
        .await
    }

    async fn image_history(&self, id: &str) -> AppResult<serde_json::Value> {
        self.record("image_history", Read, vec![], self.inner.image_history(id))
            .await
    }

    async fn inspect_image(&self, id: &str) -> AppResult<serde_json::Value> {
        self.record("inspect_image", Read, vec![], self.inner.inspect_image(id))
            .await
    }

    async fn prune_images(&self) -> AppResult<u64> {
        self.record("prune_images", Write, vec![], self.inner.prune_images())
            .await
    }

    async fn export_image(&self, reference: &str) -> AppResult<ByteStream> {
        self.record(
            "export_image",
            Read,
            vec![],
            self.inner.export_image(reference),
        )
        .await
    }

    async fn import_image(&self, tar: ByteStream) -> AppResult<ImportStream> {
        self.record("import_image", Write, vec![], self.inner.import_image(tar))
            .await
    }

    // -- networks ----------------------------------------------------------

    async fn list_networks(&self) -> AppResult<Vec<Network>> {
        self.record("list_networks", Read, vec![], self.inner.list_networks())
            .await
    }

    async fn create_network(&self, name: &str, driver: &str, internal: bool) -> AppResult<String> {
        let shown = args([
            ("name", name.to_string()),
            ("driver", driver.to_string()),
            ("internal", internal.to_string()),
        ]);
        self.record(
            "create_network",
            Write,
            shown,
            self.inner.create_network(name, driver, internal),
        )
        .await
    }

    async fn remove_network(&self, id: &str) -> AppResult<()> {
        self.record(
            "remove_network",
            Write,
            vec![],
            self.inner.remove_network(id),
        )
        .await
    }

    async fn inspect_network(&self, id: &str) -> AppResult<serde_json::Value> {
        self.record(
            "inspect_network",
            Read,
            vec![],
            self.inner.inspect_network(id),
        )
        .await
    }

    async fn connect_network(&self, network: &str, container: &str) -> AppResult<()> {
        let shown = args([("container", container.to_string())]);
        self.record(
            "connect_network",
            Write,
            shown,
            self.inner.connect_network(network, container),
        )
        .await
    }

    async fn disconnect_network(&self, network: &str, container: &str) -> AppResult<()> {
        let shown = args([("container", container.to_string())]);
        self.record(
            "disconnect_network",
            Write,
            shown,
            self.inner.disconnect_network(network, container),
        )
        .await
    }

    // -- volumes -----------------------------------------------------------

    async fn list_volumes(&self) -> AppResult<Vec<Volume>> {
        self.record("list_volumes", Read, vec![], self.inner.list_volumes())
            .await
    }

    async fn create_volume(&self, name: &str, driver: Option<&str>) -> AppResult<Volume> {
        let shown = args([
            ("name", name.to_string()),
            ("driver", driver.unwrap_or("local").to_string()),
        ]);
        self.record(
            "create_volume",
            Write,
            shown,
            self.inner.create_volume(name, driver),
        )
        .await
    }

    async fn remove_volume(&self, name: &str, force: bool) -> AppResult<()> {
        self.record(
            "remove_volume",
            Write,
            vec![],
            self.inner.remove_volume(name, force),
        )
        .await
    }

    async fn inspect_volume(&self, name: &str) -> AppResult<serde_json::Value> {
        self.record(
            "inspect_volume",
            Read,
            vec![],
            self.inner.inspect_volume(name),
        )
        .await
    }

    async fn prune_volumes(&self) -> AppResult<u64> {
        self.record("prune_volumes", Write, vec![], self.inner.prune_volumes())
            .await
    }

    // -- compose -----------------------------------------------------------
    //
    // The only operations that pass a command in rather than capturing one,
    // because compose is the one remaining subprocess and there the argv is
    // what happened.

    async fn compose_services(&self, project_dir: &str) -> AppResult<Vec<ComposeService>> {
        let argv = self.compose_argv().join(" ");
        self.record_argv(
            "compose_services",
            Read,
            format!("{argv} ps --format json"),
            args([("cwd", project_dir.to_string())]),
            self.inner.compose_services(project_dir),
        )
        .await
    }

    async fn compose_up(&self, project_dir: &str) -> AppResult<String> {
        let argv = self.compose_argv().join(" ");
        self.record_argv(
            "compose_up",
            Write,
            format!("{argv} up -d"),
            args([("cwd", project_dir.to_string())]),
            self.inner.compose_up(project_dir),
        )
        .await
    }

    async fn compose_down(&self, project_dir: &str) -> AppResult<String> {
        let argv = self.compose_argv().join(" ");
        self.record_argv(
            "compose_down",
            Write,
            format!("{argv} down"),
            args([("cwd", project_dir.to_string())]),
            self.inner.compose_down(project_dir),
        )
        .await
    }

    async fn compose_restart(&self, project_dir: &str, service: Option<&str>) -> AppResult<String> {
        let argv = self.compose_argv().join(" ");
        let suffix = service.map(|s| format!(" {s}")).unwrap_or_default();
        self.record_argv(
            "compose_restart",
            Write,
            format!("{argv} restart{suffix}"),
            args([("cwd", project_dir.to_string())]),
            self.inner.compose_restart(project_dir, service),
        )
        .await
    }

    async fn compose_exec(
        &self,
        project_dir: &str,
        action: ComposeAction,
        service: Option<&str>,
    ) -> AppResult<compose::LineStream> {
        let argv = self.compose_argv().join(" ");
        let verb = action.argv().join(" ");
        let suffix = service.map(|s| format!(" {s}")).unwrap_or_default();
        self.record_argv(
            "compose_exec",
            Write,
            format!("{argv} {verb}{suffix}"),
            args([("cwd", project_dir.to_string())]),
            self.inner.compose_exec(project_dir, action, service),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_secret_looking_env_values() {
        for entry in [
            "DB_PASSWORD=hunter2",
            "API_TOKEN=abc",
            "AWS_SECRET_ACCESS_KEY=xyz",
            "GITHUB_AUTH=zzz",
            "apikey=123",
            "KEY=abc",
            "SSH_KEY=abc",
        ] {
            let out = redact_env(entry);
            assert!(out.contains("••••"), "not redacted: {entry} -> {out}");
            assert!(
                !out.contains("hunter2") && !out.contains("abc") && !out.contains("xyz"),
                "value leaked: {out}"
            );
        }
    }

    #[test]
    fn leaves_ordinary_env_alone() {
        for entry in ["LOG_LEVEL=debug", "PORT=8080", "SORT_KEYS=true", "TZ=UTC"] {
            assert_eq!(redact_env(entry), entry, "wrongly redacted {entry}");
        }
    }

    /// A bare value with no `=` must pass through rather than panic.
    #[test]
    fn tolerates_malformed_entries() {
        assert_eq!(redact_env("NOTANASSIGNMENT"), "NOTANASSIGNMENT");
        assert_eq!(redact_env(""), "");
        // An empty secret has nothing to hide and reads better unmasked.
        assert_eq!(redact_env("PASSWORD="), "PASSWORD=");
    }

    #[test]
    fn ring_buffer_drops_oldest_and_reports_newest_first() {
        let log = ActivityLog::new();
        for i in 0..(CAPACITY + 10) {
            log.push(ActivityEntry {
                seq: i as u64,
                at: 0,
                runtime: RuntimeKind::Docker,
                op: "list_images".into(),
                kind: Read,
                requests: vec!["GET /v1.51/images/json".into()],
                args: vec![],
                duration_ms: 0,
                error: None,
            });
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), CAPACITY, "buffer should stay capped");
        assert_eq!(snap[0].seq, (CAPACITY + 9) as u64, "newest must be first");
        assert_eq!(snap[CAPACITY - 1].seq, 10, "oldest ten should be gone");

        log.clear();
        assert!(log.snapshot().is_empty());
    }
}
