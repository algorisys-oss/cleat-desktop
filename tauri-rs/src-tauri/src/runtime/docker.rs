//! Docker backend.

use crate::error::{AppError, AppResult};
use crate::model::{
    ComposeService, Container, CreateContainerRequest, Image, Network, RuntimeInfo, RuntimeKind,
    Stats, SystemSummary, Volume,
};
use crate::runtime::compose;
use crate::runtime::engine::{parse_systemctl_units, validate_service_name, Engine};
use crate::runtime::wire;
use crate::runtime::{
    ByteStream, ContainerRuntime, ExecAttach, ImportStream, LogStream, PullStream, StatsStream,
};
use async_trait::async_trait;
use bollard::Docker;

pub struct DockerRuntime {
    engine: Engine,
}

impl DockerRuntime {
    pub async fn connect() -> AppResult<Self> {
        // Honours DOCKER_HOST when set, otherwise the local socket / named pipe.
        // Instrumented immediately: the activity log reports the requests this
        // client issues, so there must be no window in which it is unhooked.
        let docker = wire::instrument(
            Docker::connect_with_defaults()
                .map_err(|e| AppError::RuntimeUnavailable(format!("Docker: {e}")))?,
        );
        docker
            .ping()
            .await
            .map_err(|e| AppError::RuntimeUnavailable(format!("Docker daemon unreachable: {e}")))?;
        // Only a plain unix socket enables the lenient fallback; TCP/SSH hosts
        // keep the typed path, which is correct for a real Docker daemon anyway.
        let socket = std::env::var("DOCKER_HOST")
            .ok()
            .and_then(|h| h.strip_prefix("unix://").map(str::to_string))
            .or_else(|| {
                let default = "/var/run/docker.sock";
                std::path::Path::new(default)
                    .exists()
                    .then(|| default.to_string())
            });

        Ok(Self {
            engine: match socket {
                Some(path) => Engine::with_socket(docker, RuntimeKind::Docker, path),
                None => Engine::new(docker, RuntimeKind::Docker),
            },
        })
    }
}

/// Is Docker present on this machine at all?
///
/// The CLI, the default socket, or an explicit DOCKER_HOST each count. A remote
/// daemon via DOCKER_HOST is "installed" even with no local binary.
fn installed() -> bool {
    crate::runtime::binary_on_path("docker")
        || std::path::Path::new("/var/run/docker.sock").exists()
        || std::env::var_os("DOCKER_HOST").is_some()
}

/// Probe Docker for the runtime switcher without committing to a connection.
pub async fn probe() -> RuntimeInfo {
    let installed = installed();
    let unavailable = |detail: String| RuntimeInfo {
        kind: RuntimeKind::Docker,
        available: false,
        installed,
        version: None,
        api_version: None,
        detail: Some(detail),
    };

    match Docker::connect_with_defaults() {
        Ok(docker) => match docker.version().await {
            Ok(v) => RuntimeInfo {
                kind: RuntimeKind::Docker,
                available: true,
                installed: true,
                version: v.version,
                api_version: v.api_version,
                detail: None,
            },
            Err(e) if installed => unavailable(format!(
                "daemon unreachable: {e}. Try `sudo systemctl start docker`"
            )),
            Err(e) => unavailable(format!("daemon unreachable: {e}")),
        },
        Err(e) => unavailable(e.to_string()),
    }
}

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Docker
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
        self.engine.pause_container(id).await
    }

    async fn unpause_container(&self, id: &str) -> AppResult<()> {
        self.engine.unpause_container(id).await
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

    async fn exec_start(
        &self,
        id: &str,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> AppResult<ExecAttach> {
        self.engine.exec_start(id, argv, cols, rows).await
    }

    async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        self.engine.exec_resize(exec_id, cols, rows).await
    }

    async fn detect_shell(&self, id: &str) -> AppResult<String> {
        self.engine.detect_shell(id).await
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
        // Whitelist the verb and constrain the unit name; neither can become a
        // separate command because there is no shell in `exec_capture`.
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
        // Compose v2 subcommand. The Electron app called `docker-compose`,
        // the v1 Python tool, which reached end of life in 2023.
        vec!["docker".into(), "compose".into()]
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
