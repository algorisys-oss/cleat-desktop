//! Podman backend.
//!
//! Podman exposes a Docker-compatible REST API, so the Engine client is reused
//! wholesale. What genuinely differs is handled here:
//!
//! * **Socket discovery** — rootless Podman listens on
//!   `$XDG_RUNTIME_DIR/podman/podman.sock`, not `/var/run/docker.sock`, and the
//!   service may not be running even when the binary is installed.
//! * **Compose** — `podman compose` (v4.7+) or the separate `podman-compose`.
//! * **Pause/unpause** — unsupported for rootless containers on cgroups v1;
//!   surfaced as a clear error rather than an opaque 500.

use crate::error::{AppError, AppResult};
use crate::model::{
    ComposeService, Container, CreateContainerRequest, Image, Network, RuntimeInfo, RuntimeKind,
    Stats, SystemSummary, Volume,
};
use crate::runtime::compose;
use crate::runtime::engine::{parse_systemctl_units, validate_service_name, Engine};
use crate::runtime::{
    ByteStream, ContainerRuntime, ImportStream, LogStream, PullStream, StatsStream,
};
use async_trait::async_trait;
use bollard::Docker;
use std::path::Path;

pub struct PodmanRuntime {
    engine: Engine,
    compose_argv: Vec<String>,
}

/// Locate a Podman API socket, or explain why there isn't one.
///
/// Deliberately *not* `Docker::connect_with_podman_defaults()`: that helper
/// falls back to `/var/run/docker.sock` when no Podman socket exists, so on a
/// Docker-only machine Podman would report itself available and then silently
/// serve Docker's containers under the Podman label.
fn podman_socket_path() -> AppResult<String> {
    // Podman's own environment variable takes precedence.
    if let Ok(host) = std::env::var("CONTAINER_HOST") {
        if let Some(path) = host.strip_prefix("unix://") {
            if Path::new(path).exists() {
                return Ok(path.to_string());
            }
            return Err(AppError::RuntimeUnavailable(format!(
                "CONTAINER_HOST points at {path}, which does not exist"
            )));
        }
        return Err(AppError::RuntimeUnavailable(format!(
            "CONTAINER_HOST '{host}' is not a unix:// socket; remote Podman is not supported yet"
        )));
    }

    // Rootless socket, the common case.
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .unwrap_or_else(|| format!("/run/user/{}", current_uid()));
    let rootless = format!("{runtime_dir}/podman/podman.sock");
    if Path::new(&rootless).exists() {
        return Ok(rootless);
    }

    // Rootful socket.
    const ROOTFUL: &str = "/run/podman/podman.sock";
    if Path::new(ROOTFUL).exists() {
        return Ok(ROOTFUL.to_string());
    }

    Err(AppError::RuntimeUnavailable(
        "no Podman API socket found. Start it with `systemctl --user start podman.socket`".into(),
    ))
}

/// Current uid, without pulling in the libc crate for a single call.
/// `/proc/self` is owned by the running user, so its metadata carries the uid.
fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
}

/// Connect to whichever Podman socket this system actually exposes.
fn connect_client() -> AppResult<(Docker, String)> {
    let socket = podman_socket_path()?;
    let client = Docker::connect_with_unix(&socket, 120, bollard::API_DEFAULT_VERSION)
        .map_err(|e| AppError::RuntimeUnavailable(format!("Podman socket {socket}: {e}")))?;
    Ok((client, socket))
}

/// Confirm the daemon behind the socket really is Podman.
///
/// Podman advertises itself in the `components` array of `/version`; Docker
/// advertises "Engine". Checking protects against a socket that has been
/// pointed at a Docker daemon.
fn is_podman(version: &bollard::models::SystemVersion) -> bool {
    if let Some(components) = &version.components {
        if components
            .iter()
            .any(|c| c.name.to_ascii_lowercase().contains("podman"))
        {
            return true;
        }
    }
    version
        .platform
        .as_ref()
        .map(|p| p.name.to_ascii_lowercase().contains("podman"))
        .unwrap_or(false)
}

/// Pick the compose front-end this Podman install provides.
///
/// Resolved once at connect time rather than per call, so a missing binary is
/// reported when the runtime is selected instead of on first compose action.
async fn detect_compose_argv() -> Vec<String> {
    // `podman compose` delegates to whatever provider is installed and is the
    // preferred spelling on Podman 4.7+.
    if which_ok("podman").await {
        return vec!["podman".into(), "compose".into()];
    }
    vec!["podman-compose".into()]
}

async fn which_ok(bin: &str) -> bool {
    tokio::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

impl PodmanRuntime {
    pub async fn connect() -> AppResult<Self> {
        let (docker, socket) = connect_client()?;
        let version = docker.version().await.map_err(|e| {
            AppError::RuntimeUnavailable(format!(
                "Podman API socket unreachable: {e}. Start it with `systemctl --user start podman.socket`"
            ))
        })?;
        if !is_podman(&version) {
            return Err(AppError::RuntimeUnavailable(
                "the socket found is served by Docker, not Podman".into(),
            ));
        }
        Ok(Self {
            // Podman needs the lenient fallback: it reports states outside
            // Docker's schema, which breaks strict deserialisation.
            engine: Engine::with_socket(docker, RuntimeKind::Podman, socket),
            compose_argv: detect_compose_argv().await,
        })
    }
}

/// Is Podman present on this machine at all?
fn installed() -> bool {
    crate::runtime::binary_on_path("podman")
        || std::env::var_os("CONTAINER_HOST").is_some()
        || podman_socket_path().is_ok()
}

/// Probe Podman for the runtime switcher.
pub async fn probe() -> RuntimeInfo {
    let installed = installed();
    let unavailable = |detail: String| RuntimeInfo {
        kind: RuntimeKind::Podman,
        available: false,
        installed,
        version: None,
        api_version: None,
        detail: Some(detail),
    };

    match connect_client() {
        Ok((docker, _)) => match docker.version().await {
            Ok(v) if is_podman(&v) => RuntimeInfo {
                kind: RuntimeKind::Podman,
                available: true,
                installed: true,
                version: v.version,
                api_version: v.api_version,
                detail: None,
            },
            Ok(_) => unavailable("the socket found is served by Docker, not Podman".into()),
            Err(e) => unavailable(format!(
                "API socket unreachable: {e}. Try `systemctl --user start podman.socket`"
            )),
        },
        Err(e) => unavailable(e.message()),
    }
}

#[async_trait]
impl ContainerRuntime for PodmanRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Podman
    }

    async fn system_summary(&self) -> AppResult<SystemSummary> {
        self.engine.system_summary().await
    }

    async fn list_containers(&self, all: bool) -> AppResult<Vec<Container>> {
        self.engine.list_containers(all).await
    }

    async fn inspect_container(&self, id: &str) -> AppResult<serde_json::Value> {
        self.engine.inspect_container(id).await
    }

    async fn start_container(&self, id: &str) -> AppResult<()> {
        self.engine.start_container(id).await
    }

    async fn stop_container(&self, id: &str) -> AppResult<()> {
        self.engine.stop_container(id).await
    }

    async fn restart_container(&self, id: &str) -> AppResult<()> {
        self.engine.restart_container(id).await
    }

    async fn pause_container(&self, id: &str) -> AppResult<()> {
        self.engine.pause_container(id).await.map_err(annotate_pause)
    }

    async fn unpause_container(&self, id: &str) -> AppResult<()> {
        self.engine
            .unpause_container(id)
            .await
            .map_err(annotate_pause)
    }

    async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> AppResult<()> {
        self.engine.remove_container(id, force, volumes).await
    }

    async fn create_container(&self, req: CreateContainerRequest) -> AppResult<String> {
        self.engine.create_container(req).await
    }

    async fn container_health(&self, id: &str) -> AppResult<String> {
        self.engine.container_health(id).await
    }

    async fn container_logs(&self, id: &str, tail: i64, timestamps: bool) -> AppResult<String> {
        self.engine.container_logs(id, tail, timestamps).await
    }

    async fn follow_logs(&self, id: &str, tail: i64) -> AppResult<LogStream> {
        self.engine.follow_logs(id, tail).await
    }

    async fn container_stats(&self, id: &str) -> AppResult<Stats> {
        self.engine.container_stats(id).await
    }

    async fn stream_stats(&self, id: &str) -> AppResult<StatsStream> {
        self.engine.stream_stats(id).await
    }

    async fn list_container_services(&self, id: &str) -> AppResult<Vec<(String, String)>> {
        let out = self
            .engine
            .exec_capture(
                id,
                vec![
                    "systemctl".into(),
                    "list-units".into(),
                    "--type=service".into(),
                    "--no-pager".into(),
                    "--plain".into(),
                    "--no-legend".into(),
                ],
            )
            .await?;
        Ok(parse_systemctl_units(&out))
    }

    async fn control_container_service(
        &self,
        id: &str,
        service: &str,
        action: &str,
    ) -> AppResult<String> {
        let action = match action {
            "start" | "stop" | "restart" | "status" => action,
            other => return Err(AppError::Invalid(format!("unsupported action '{other}'"))),
        };
        validate_service_name(service)?;
        self.engine
            .exec_capture(id, vec!["systemctl".into(), action.into(), service.into()])
            .await
    }

    async fn list_images(&self) -> AppResult<Vec<Image>> {
        self.engine.list_images().await
    }

    async fn pull_image(&self, image: &str) -> AppResult<PullStream> {
        self.engine.pull_image(image).await
    }

    async fn remove_image(&self, id: &str, force: bool) -> AppResult<()> {
        self.engine.remove_image(id, force).await
    }

    async fn image_history(&self, id: &str) -> AppResult<serde_json::Value> {
        self.engine.image_history(id).await
    }

    async fn inspect_image(&self, id: &str) -> AppResult<serde_json::Value> {
        self.engine.inspect_image(id).await
    }

    async fn prune_images(&self) -> AppResult<u64> {
        self.engine.prune_images().await
    }

    async fn export_image(&self, reference: &str) -> AppResult<ByteStream> {
        self.engine.export_image(reference).await
    }

    async fn import_image(&self, tar: ByteStream) -> AppResult<ImportStream> {
        self.engine.import_image(tar).await
    }

    async fn list_networks(&self) -> AppResult<Vec<Network>> {
        self.engine.list_networks().await
    }

    async fn create_network(&self, name: &str, driver: &str, internal: bool) -> AppResult<String> {
        // Podman's default network backend (netavark) has no "overlay" driver;
        // fail with the reason instead of a bare 500 from the API.
        if driver == "overlay" {
            return Err(AppError::Invalid(
                "Podman does not support the overlay driver; use bridge or macvlan".into(),
            ));
        }
        self.engine.create_network(name, driver, internal).await
    }

    async fn remove_network(&self, id: &str) -> AppResult<()> {
        self.engine.remove_network(id).await
    }

    async fn inspect_network(&self, id: &str) -> AppResult<serde_json::Value> {
        self.engine.inspect_network(id).await
    }

    async fn connect_network(&self, network: &str, container: &str) -> AppResult<()> {
        self.engine.connect_network(network, container).await
    }

    async fn disconnect_network(&self, network: &str, container: &str) -> AppResult<()> {
        self.engine.disconnect_network(network, container).await
    }

    async fn list_volumes(&self) -> AppResult<Vec<Volume>> {
        self.engine.list_volumes().await
    }

    async fn create_volume(&self, name: &str, driver: Option<&str>) -> AppResult<Volume> {
        self.engine.create_volume(name, driver).await
    }

    async fn remove_volume(&self, name: &str, force: bool) -> AppResult<()> {
        self.engine.remove_volume(name, force).await
    }

    async fn inspect_volume(&self, name: &str) -> AppResult<serde_json::Value> {
        self.engine.inspect_volume(name).await
    }

    async fn prune_volumes(&self) -> AppResult<u64> {
        self.engine.prune_volumes().await
    }

    fn compose_argv(&self) -> Vec<String> {
        self.compose_argv.clone()
    }

    async fn compose_services(&self, project_dir: &str) -> AppResult<Vec<ComposeService>> {
        let dir = compose::resolve_project_dir(project_dir)?;
        compose::find_compose_file(&dir)?;
        let out = compose::run(&self.compose_argv(), &["ps", "--format", "json"], &dir).await?;
        let fallback = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        Ok(compose::parse_ps_json(&out, &fallback))
    }

    async fn compose_up(&self, project_dir: &str) -> AppResult<String> {
        let dir = compose::resolve_project_dir(project_dir)?;
        compose::find_compose_file(&dir)?;
        compose::run(&self.compose_argv(), &["up", "-d"], &dir).await
    }

    async fn compose_down(&self, project_dir: &str) -> AppResult<String> {
        let dir = compose::resolve_project_dir(project_dir)?;
        compose::run(&self.compose_argv(), &["down"], &dir).await
    }

    async fn compose_restart(&self, project_dir: &str, service: Option<&str>) -> AppResult<String> {
        let dir = compose::resolve_project_dir(project_dir)?;
        let mut args = vec!["restart"];
        if let Some(svc) = service {
            validate_service_name(svc)?;
            args.push(svc);
        }
        compose::run(&self.compose_argv(), &args, &dir).await
    }

    async fn compose_exec(
        &self,
        project_dir: &str,
        action: crate::runtime::ComposeAction,
        service: Option<&str>,
    ) -> AppResult<compose::LineStream> {
        let dir = compose::resolve_project_dir(project_dir)?;
        if action == crate::runtime::ComposeAction::Up {
            compose::find_compose_file(&dir)?;
        }
        let mut args = action.argv();
        if let Some(svc) = service {
            if !action.accepts_service() {
                return Err(AppError::Invalid(format!(
                    "'{action:?}' applies to the whole project, not a single service"
                )));
            }
            validate_service_name(svc)?;
            args.push(svc.to_string());
        }
        compose::stream(&self.compose_argv(), &args, &dir)
    }
}

/// Podman refuses pause on rootless cgroups v1; make that legible.
fn annotate_pause(e: AppError) -> AppError {
    let msg = e.message();
    if msg.contains("cgroup") || msg.contains("not supported") || msg.contains("rootless") {
        AppError::Conflict(format!(
            "Podman cannot pause this container ({msg}). Rootless pause requires cgroups v2."
        ))
    } else {
        e
    }
}
