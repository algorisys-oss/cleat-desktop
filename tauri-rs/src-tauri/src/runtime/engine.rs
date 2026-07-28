//! Shared Docker Engine API client.
//!
//! Docker and Podman (via its compat endpoint) both speak this API, so both
//! backends delegate here rather than duplicating ~500 lines of mapping. The
//! per-runtime impls own only what genuinely differs.

use crate::error::{AppError, AppResult};
use crate::model::{
    Container, CreateContainerRequest, Image, LogLine, Network, PortBinding, PullProgress,
    RuntimeKind, Stats, SystemSummary, Volume,
};
use crate::runtime::{ExecAttach, LogStream, PullStream, StatsStream};
use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkCreateRequest, PortBinding as PB,
    RestartPolicy, RestartPolicyNameEnum, VolumeCreateRequest,
};
use bollard::query_parameters as qp;
use bollard::Docker;
use futures_util::stream::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub struct Engine {
    pub docker: Docker,
    pub kind: RuntimeKind,
    /// Unix socket path, when we know it. Enables the lenient fallback in
    /// [`list_containers`](Self::list_containers) for runtimes that report
    /// values outside Docker's schema.
    pub socket: Option<String>,
}

impl Engine {
    pub fn new(docker: Docker, kind: RuntimeKind) -> Self {
        Self {
            docker,
            kind,
            socket: None,
        }
    }

    /// Same, but able to fall back to raw JSON when strict parsing fails.
    pub fn with_socket(docker: Docker, kind: RuntimeKind, socket: impl Into<String>) -> Self {
        Self {
            docker,
            kind,
            socket: Some(socket.into()),
        }
    }

    // ---------------------------------------------------------------- system

    pub async fn system_summary(&self) -> AppResult<SystemSummary> {
        let (version, info) = tokio::try_join!(self.docker.version(), self.docker.info())?;
        Ok(SystemSummary {
            runtime: self.kind,
            version: version.version.unwrap_or_else(|| "unknown".into()),
            api_version: version.api_version.unwrap_or_else(|| "unknown".into()),
            os: version.os.unwrap_or_else(|| "unknown".into()),
            arch: version.arch.unwrap_or_else(|| "unknown".into()),
            kernel: info.kernel_version,
            storage_driver: info.driver,
            cpus: info.ncpu.unwrap_or(0) as i64,
            memory: info.mem_total.unwrap_or(0),
            containers_running: info.containers_running.unwrap_or(0) as i64,
            containers_paused: info.containers_paused.unwrap_or(0) as i64,
            containers_stopped: info.containers_stopped.unwrap_or(0) as i64,
            images: info.images.unwrap_or(0) as i64,
        })
    }

    // ------------------------------------------------------------ containers

    pub async fn list_containers(&self, all: bool) -> AppResult<Vec<Container>> {
        let opts = qp::ListContainersOptions {
            all,
            ..Default::default()
        };

        let summaries = match self.docker.list_containers(Some(opts)).await {
            Ok(summaries) => summaries,
            // Podman reports states outside Docker's enum (`stopped`), which
            // makes serde reject the whole response. Re-fetch untyped rather
            // than showing the user a JSON error for the entire list.
            Err(bollard::errors::Error::JsonDataError { .. }) if self.socket.is_some() => {
                return self.list_containers_lenient(all).await;
            }
            Err(e) => return Err(AppError::Engine(e)),
        };

        Ok(summaries
            .into_iter()
            .map(|c| {
                let names: Vec<String> = c
                    .names
                    .unwrap_or_default()
                    .into_iter()
                    .map(|n| n.trim_start_matches('/').to_string())
                    .collect();
                let labels = c.labels.unwrap_or_default();
                Container {
                    id: c.id.unwrap_or_default(),
                    name: names.first().cloned().unwrap_or_else(|| "<unnamed>".into()),
                    names,
                    image: c.image.unwrap_or_default(),
                    image_id: c.image_id.unwrap_or_default(),
                    command: c.command,
                    created: c.created.unwrap_or(0),
                    state: crate::runtime::raw::normalize_state(
                        &c.state
                            .and_then(|s| {
                                serde_json::to_value(s)
                                    .ok()
                                    .and_then(|v| v.as_str().map(str::to_string))
                            })
                            .unwrap_or_default(),
                    ),
                    status: c.status.unwrap_or_default(),
                    ports: c
                        .ports
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| PortBinding {
                            ip: p.ip,
                            private_port: p.private_port,
                            public_port: p.public_port,
                            protocol: p.typ.and_then(|t| {
                                serde_json::to_value(t)
                                    .ok()
                                    .and_then(|v| v.as_str().map(str::to_string))
                            }),
                        })
                        .collect(),
                    health: normalize_health(c.health.and_then(|h| {
                        serde_json::to_value(h)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                    })),
                    compose_project: labels.get("com.docker.compose.project").cloned(),
                    labels,
                }
            })
            .collect())
    }

    /// Untyped container listing, used when strict parsing fails.
    ///
    /// Maps the same fields as the typed path, but tolerates any string in
    /// `State` — see [`crate::runtime::raw`] for why that is necessary.
    async fn list_containers_lenient(&self, all: bool) -> AppResult<Vec<Container>> {
        let socket = self
            .socket
            .as_deref()
            .ok_or_else(|| AppError::Other("no socket path for lenient listing".into()))?;

        let path = format!("/containers/json?all={}", if all { "1" } else { "0" });
        let value = crate::runtime::raw::get_json(socket, &path).await?;
        let entries = value
            .as_array()
            .ok_or_else(|| AppError::Other("expected an array of containers".into()))?;

        Ok(entries
            .iter()
            .map(|c| {
                let str_at = |k: &str| {
                    c.get(k)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                };

                let names: Vec<String> = c
                    .get("Names")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| n.as_str())
                            .map(|n| n.trim_start_matches('/').to_string())
                            .collect()
                    })
                    .unwrap_or_default();

                let labels: std::collections::HashMap<String, String> = c
                    .get("Labels")
                    .and_then(|v| v.as_object())
                    .map(|o| {
                        o.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();

                let ports = c
                    .get("Ports")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| {
                                Some(PortBinding {
                                    ip: p.get("IP").and_then(|v| v.as_str()).map(str::to_string),
                                    private_port: p.get("PrivatePort")?.as_u64()? as u16,
                                    public_port: p
                                        .get("PublicPort")
                                        .and_then(|v| v.as_u64())
                                        .map(|n| n as u16),
                                    protocol: p
                                        .get("Type")
                                        .and_then(|v| v.as_str())
                                        .map(str::to_string),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                Container {
                    id: str_at("Id"),
                    name: names.first().cloned().unwrap_or_else(|| "<unnamed>".into()),
                    names,
                    image: str_at("Image"),
                    image_id: str_at("ImageID"),
                    command: c
                        .get("Command")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    created: c.get("Created").and_then(|v| v.as_i64()).unwrap_or(0),
                    state: crate::runtime::raw::normalize_state(
                        c.get("State").and_then(|v| v.as_str()).unwrap_or(""),
                    ),
                    status: str_at("Status"),
                    ports,
                    health: normalize_health(
                        c.get("Health").and_then(|v| v.as_str()).map(str::to_string),
                    ),
                    compose_project: labels.get("com.docker.compose.project").cloned(),
                    labels,
                }
            })
            .collect())
    }

    pub async fn inspect_container(&self, id: &str) -> AppResult<serde_json::Value> {
        let data = self
            .docker
            .inspect_container(id, None::<qp::InspectContainerOptions>)
            .await?;
        Ok(serde_json::to_value(data).map_err(|e| AppError::Other(e.to_string()))?)
    }

    pub async fn start_container(&self, id: &str) -> AppResult<()> {
        self.docker
            .start_container(id, None::<qp::StartContainerOptions>)
            .await?;
        Ok(())
    }

    pub async fn stop_container(&self, id: &str) -> AppResult<()> {
        self.docker
            .stop_container(id, None::<qp::StopContainerOptions>)
            .await?;
        Ok(())
    }

    pub async fn restart_container(&self, id: &str) -> AppResult<()> {
        self.docker
            .restart_container(id, None::<qp::RestartContainerOptions>)
            .await?;
        Ok(())
    }

    pub async fn pause_container(&self, id: &str) -> AppResult<()> {
        self.docker.pause_container(id).await?;
        Ok(())
    }

    pub async fn unpause_container(&self, id: &str) -> AppResult<()> {
        self.docker.unpause_container(id).await?;
        Ok(())
    }

    pub async fn remove_container(&self, id: &str, force: bool, volumes: bool) -> AppResult<()> {
        let opts = qp::RemoveContainerOptions {
            force,
            v: volumes,
            link: false,
        };
        self.docker.remove_container(id, Some(opts)).await?;
        Ok(())
    }

    pub async fn container_health(&self, id: &str) -> AppResult<String> {
        let data = self
            .docker
            .inspect_container(id, None::<qp::InspectContainerOptions>)
            .await?;
        Ok(normalize_health(
            data.state
                .and_then(|s| s.health)
                .and_then(|h| h.status)
                .and_then(|s| {
                    serde_json::to_value(s)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                }),
        ))
    }

    pub async fn create_container(&self, req: CreateContainerRequest) -> AppResult<String> {
        if req.image.trim().is_empty() {
            return Err(AppError::Invalid("image is required".into()));
        }

        let mut port_bindings: HashMap<String, Option<Vec<PB>>> = HashMap::new();
        let mut exposed: Vec<String> = Vec::new();
        for spec in &req.ports {
            let (host, container) = parse_port_spec(spec)?;
            exposed.push(container.clone());
            port_bindings.insert(
                container,
                Some(vec![PB {
                    host_ip: None,
                    host_port: Some(host),
                }]),
            );
        }

        let restart_policy = req.restart_policy.as_deref().map(|p| RestartPolicy {
            name: Some(match p {
                "always" => RestartPolicyNameEnum::ALWAYS,
                "unless-stopped" => RestartPolicyNameEnum::UNLESS_STOPPED,
                "on-failure" => RestartPolicyNameEnum::ON_FAILURE,
                _ => RestartPolicyNameEnum::NO,
            }),
            maximum_retry_count: None,
        });

        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            binds: if req.volumes.is_empty() {
                None
            } else {
                Some(req.volumes.clone())
            },
            auto_remove: Some(req.auto_remove),
            restart_policy,
            network_mode: req.network.clone(),
            ..Default::default()
        };

        // Exact argv straight through; no shell, and no splitting that would
        // silently reinterpret a quoted argument.
        let cmd = Some(req.command.clone()).filter(|v| !v.is_empty());

        let body = ContainerCreateBody {
            image: Some(req.image.clone()),
            env: if req.env.is_empty() {
                None
            } else {
                Some(req.env.clone())
            },
            cmd,
            exposed_ports: if exposed.is_empty() {
                None
            } else {
                Some(exposed)
            },
            host_config: Some(host_config),
            ..Default::default()
        };

        let opts = req.name.as_ref().map(|n| qp::CreateContainerOptions {
            name: Some(n.clone()),
            platform: String::new(),
        });

        let res = self.docker.create_container(opts, body).await?;
        Ok(res.id)
    }

    // ------------------------------------------------------------------ logs

    pub async fn container_logs(&self, id: &str, tail: i64, timestamps: bool) -> AppResult<String> {
        let opts = qp::LogsOptions {
            follow: false,
            stdout: true,
            stderr: true,
            timestamps,
            tail: tail.to_string(),
            ..Default::default()
        };
        let mut stream = self.docker.logs(id, Some(opts));
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            out.push_str(&chunk?.to_string());
        }
        Ok(out)
    }

    /// Follow mode. Each frame becomes a [`LogLine`] tagged with its stream and
    /// a monotonic sequence number so the UI can order and de-duplicate.
    pub async fn follow_logs(&self, id: &str, tail: i64) -> AppResult<LogStream> {
        let opts = qp::LogsOptions {
            follow: true,
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            ..Default::default()
        };
        let id = id.to_string();
        let seq = Arc::new(AtomicU64::new(0));
        let stream = self.docker.logs(&id, Some(opts)).map(move |item| {
            let seq = seq.fetch_add(1, Ordering::Relaxed);
            match item {
                Ok(out) => {
                    let (stream_name, message) = match &out {
                        bollard::container::LogOutput::StdErr { message } => {
                            ("stderr", String::from_utf8_lossy(message).to_string())
                        }
                        bollard::container::LogOutput::StdOut { message } => {
                            ("stdout", String::from_utf8_lossy(message).to_string())
                        }
                        bollard::container::LogOutput::Console { message }
                        | bollard::container::LogOutput::StdIn { message } => {
                            ("stdout", String::from_utf8_lossy(message).to_string())
                        }
                    };
                    Ok(LogLine {
                        container_id: id.clone(),
                        stream: stream_name.to_string(),
                        message: message.trim_end_matches('\n').to_string(),
                        seq,
                    })
                }
                Err(e) => Err(AppError::Engine(e)),
            }
        });
        Ok(Box::pin(stream))
    }

    // ----------------------------------------------------------------- stats

    pub async fn container_stats(&self, id: &str) -> AppResult<Stats> {
        let opts = qp::StatsOptions {
            stream: false,
            one_shot: true,
        };
        let mut stream = self.docker.stats(id, Some(opts));
        match stream.next().await {
            Some(Ok(raw)) => Ok(reduce_stats(id, raw)),
            Some(Err(e)) => Err(AppError::Engine(e)),
            None => Err(AppError::NotFound(format!("no stats for container {id}"))),
        }
    }

    /// Continuous samples. `one_shot: false` makes the daemon include
    /// `precpu_stats`, without which CPU% cannot be computed at all.
    pub async fn stream_stats(&self, id: &str) -> AppResult<StatsStream> {
        let opts = qp::StatsOptions {
            stream: true,
            one_shot: false,
        };
        let id = id.to_string();
        let stream = self
            .docker
            .stats(&id, Some(opts))
            .map(move |item| match item {
                Ok(raw) => Ok(reduce_stats(&id, raw)),
                Err(e) => Err(AppError::Engine(e)),
            });
        Ok(Box::pin(stream))
    }

    // ---------------------------------------------------------------- images

    pub async fn list_images(&self) -> AppResult<Vec<Image>> {
        let opts = qp::ListImagesOptions {
            all: false,
            ..Default::default()
        };
        let images = self.docker.list_images(Some(opts)).await?;
        Ok(images
            .into_iter()
            .map(|i| Image {
                dangling: i.repo_tags.is_empty()
                    || i.repo_tags == vec!["<none>:<none>".to_string()],
                id: i.id,
                repo_tags: i.repo_tags,
                repo_digests: i.repo_digests,
                created: i.created,
                size: i.size,
                containers: i.containers,
            })
            .collect())
    }

    /// Pull with real per-layer progress.
    ///
    /// The Electron version only `console.log`'d these events server-side, so
    /// the UI never saw them; here each event is normalised into a
    /// [`PullProgress`] with an overall fraction the frontend can bind a bar to.
    pub async fn pull_image(&self, image: &str) -> AppResult<PullStream> {
        let image = image.trim().to_string();
        if image.is_empty() {
            return Err(AppError::Invalid("image name is required".into()));
        }
        // Default to :latest so the daemon doesn't pull every tag in the repo.
        let (from_image, tag) = match image.rsplit_once(':') {
            // A colon in the final path segment is a tag; one before a '/' is a
            // registry port (localhost:5000/foo), which is not a tag.
            Some((repo, tag)) if !tag.contains('/') => (repo.to_string(), tag.to_string()),
            _ => (image.clone(), "latest".to_string()),
        };

        let opts = qp::CreateImageOptions {
            from_image: Some(from_image),
            tag: Some(tag),
            ..Default::default()
        };

        let label = image.clone();
        // Per-layer byte counters, so `overall` reflects the whole pull rather
        // than whichever layer reported last.
        let layers: Arc<std::sync::Mutex<HashMap<String, (i64, i64)>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));

        let stream = self
            .docker
            .create_image(Some(opts), None, None)
            .map(move |item| match item {
                Ok(info) => {
                    let status = info.status.clone().unwrap_or_default();
                    let (current, total) = info
                        .progress_detail
                        .as_ref()
                        .map(|d| (d.current, d.total))
                        .unwrap_or((None, None));

                    let overall = {
                        let mut map = layers.lock().unwrap();
                        if let (Some(id), Some(total)) = (info.id.as_ref(), total) {
                            if total > 0 {
                                map.insert(id.clone(), (current.unwrap_or(0), total));
                            }
                        }
                        let (cur, tot) = map
                            .values()
                            .fold((0i64, 0i64), |(c, t), (lc, lt)| (c + lc, t + lt));
                        if tot > 0 {
                            Some((cur as f64 / tot as f64).clamp(0.0, 1.0))
                        } else {
                            None
                        }
                    };

                    Ok(PullProgress {
                        image: label.clone(),
                        id: info.id,
                        status,
                        current,
                        total,
                        overall,
                        done: false,
                        error: info.error_detail.and_then(|d| d.message),
                    })
                }
                Err(e) => Err(AppError::Engine(e)),
            });

        Ok(Box::pin(stream))
    }

    pub async fn remove_image(&self, id: &str, force: bool) -> AppResult<()> {
        let opts = qp::RemoveImageOptions {
            force,
            noprune: false,
            ..Default::default()
        };
        self.docker.remove_image(id, Some(opts), None).await?;
        Ok(())
    }

    pub async fn image_history(&self, id: &str) -> AppResult<serde_json::Value> {
        let history = self.docker.image_history(id).await?;
        serde_json::to_value(history).map_err(|e| AppError::Other(e.to_string()))
    }

    pub async fn inspect_image(&self, id: &str) -> AppResult<serde_json::Value> {
        let data = self.docker.inspect_image(id).await?;
        serde_json::to_value(data).map_err(|e| AppError::Other(e.to_string()))
    }

    /// Stream an image out as an uncompressed TAR archive.
    ///
    /// `GET /images/{name}/get`. The stream is consumed directly by another
    /// runtime's [`import_image`](Self::import_image), so a multi-gigabyte
    /// image never lands on disk or in memory as a whole.
    pub async fn export_image(&self, reference: &str) -> AppResult<crate::runtime::ByteStream> {
        if reference.trim().is_empty() {
            return Err(AppError::Invalid("image reference is required".into()));
        }
        let stream = self
            .docker
            .export_image(reference)
            .map(|item| item.map_err(AppError::Engine));
        Ok(Box::pin(stream))
    }

    /// Load an image from a TAR stream.
    ///
    /// `POST /images/load`. Status lines are surfaced so the UI can show which
    /// layer is being written rather than a blank bar.
    pub async fn import_image(
        &self,
        tar: crate::runtime::ByteStream,
    ) -> AppResult<crate::runtime::ImportStream> {
        let opts = qp::ImportImageOptions {
            quiet: false,
            ..Default::default()
        };
        let stream = self
            .docker
            .import_image_stream(opts, tar, None)
            .map(|item| match item {
                Ok(info) => {
                    // The daemon reports load failures in-band on a 200 stream.
                    if let Some(err) = info.error_detail.and_then(|d| d.message) {
                        return Err(AppError::Other(err));
                    }
                    Ok(info
                        .stream
                        .or(info.status)
                        .unwrap_or_default()
                        .trim()
                        .to_string())
                }
                Err(e) => Err(AppError::Engine(e)),
            });
        Ok(Box::pin(stream))
    }

    pub async fn prune_images(&self) -> AppResult<u64> {
        let res = self
            .docker
            .prune_images(None::<qp::PruneImagesOptions>)
            .await?;
        Ok(res.space_reclaimed.unwrap_or(0).max(0) as u64)
    }

    // -------------------------------------------------------------- networks

    /// List networks, each annotated with the containers attached to it.
    ///
    /// `GET /networks` does not report attachments, so the mapping is built
    /// from the container list instead. Without this the network view can only
    /// show a driver and a name, which is what the Electron version displayed.
    pub async fn list_networks(&self) -> AppResult<Vec<Network>> {
        let (networks, containers) = tokio::try_join!(
            self.docker.list_networks(None::<qp::ListNetworksOptions>),
            self.docker.list_containers(Some(qp::ListContainersOptions {
                all: true,
                ..Default::default()
            }))
        )?;

        let mut attached: HashMap<String, Vec<String>> = HashMap::new();
        for c in containers {
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();
            if let Some(nets) = c.network_settings.and_then(|ns| ns.networks) {
                for net_name in nets.keys() {
                    attached
                        .entry(net_name.clone())
                        .or_default()
                        .push(name.clone());
                }
            }
        }

        Ok(networks
            .into_iter()
            .map(|n| {
                let net_name = n.name.unwrap_or_default();
                let mut containers = attached.get(&net_name).cloned().unwrap_or_default();
                containers.sort();
                Network {
                    id: n.id.unwrap_or_default(),
                    name: net_name,
                    driver: n.driver.unwrap_or_default(),
                    scope: n.scope.unwrap_or_default(),
                    internal: n.internal.unwrap_or(false),
                    attachable: n.attachable.unwrap_or(false),
                    created: n.created.map(|d| d.to_string()),
                    containers,
                    subnets: n
                        .ipam
                        .and_then(|i| i.config)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|c| c.subnet)
                        .collect(),
                }
            })
            .collect())
    }

    pub async fn create_network(
        &self,
        name: &str,
        driver: &str,
        internal: bool,
    ) -> AppResult<String> {
        if name.trim().is_empty() {
            return Err(AppError::Invalid("network name is required".into()));
        }
        let req = NetworkCreateRequest {
            name: name.to_string(),
            driver: Some(driver.to_string()),
            internal: Some(internal),
            ..Default::default()
        };
        let res = self.docker.create_network(req).await?;
        Ok(res.id)
    }

    pub async fn remove_network(&self, id: &str) -> AppResult<()> {
        self.docker.remove_network(id).await?;
        Ok(())
    }

    pub async fn inspect_network(&self, id: &str) -> AppResult<serde_json::Value> {
        let data = self
            .docker
            .inspect_network(id, None::<qp::InspectNetworkOptions>)
            .await?;
        serde_json::to_value(data).map_err(|e| AppError::Other(e.to_string()))
    }

    pub async fn connect_network(&self, network: &str, container: &str) -> AppResult<()> {
        let req = bollard::models::NetworkConnectRequest {
            container: container.to_string(),
            endpoint_config: Some(EndpointSettings::default()),
        };
        self.docker.connect_network(network, req).await?;
        Ok(())
    }

    pub async fn disconnect_network(&self, network: &str, container: &str) -> AppResult<()> {
        let req = bollard::models::NetworkDisconnectRequest {
            container: container.to_string(),
            force: Some(false),
        };
        self.docker.disconnect_network(network, req).await?;
        Ok(())
    }

    // --------------------------------------------------------------- volumes

    pub async fn list_volumes(&self) -> AppResult<Vec<Volume>> {
        let res = self
            .docker
            .list_volumes(None::<qp::ListVolumesOptions>)
            .await?;
        Ok(res
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v| Volume {
                name: v.name,
                driver: v.driver,
                mountpoint: v.mountpoint,
                created_at: v.created_at.map(|d| d.to_string()),
                labels: v.labels,
                scope: v.scope.and_then(|s| {
                    serde_json::to_value(s)
                        .ok()
                        .and_then(|x| x.as_str().map(str::to_string))
                }),
                size: v.usage_data.map(|u| u.size),
            })
            .collect())
    }

    pub async fn create_volume(&self, name: &str, driver: Option<&str>) -> AppResult<Volume> {
        if name.trim().is_empty() {
            return Err(AppError::Invalid("volume name is required".into()));
        }
        let req = VolumeCreateRequest {
            name: Some(name.to_string()),
            driver: Some(driver.unwrap_or("local").to_string()),
            ..Default::default()
        };
        let v = self.docker.create_volume(req).await?;
        Ok(Volume {
            name: v.name,
            driver: v.driver,
            mountpoint: v.mountpoint,
            created_at: v.created_at.map(|d| d.to_string()),
            labels: v.labels,
            scope: None,
            size: None,
        })
    }

    pub async fn remove_volume(&self, name: &str, force: bool) -> AppResult<()> {
        let opts = qp::RemoveVolumeOptions { force };
        self.docker.remove_volume(name, Some(opts)).await?;
        Ok(())
    }

    pub async fn inspect_volume(&self, name: &str) -> AppResult<serde_json::Value> {
        let data = self.docker.inspect_volume(name).await?;
        serde_json::to_value(data).map_err(|e| AppError::Other(e.to_string()))
    }

    pub async fn prune_volumes(&self) -> AppResult<u64> {
        let res = self
            .docker
            .prune_volumes(None::<qp::PruneVolumesOptions>)
            .await?;
        Ok(res.space_reclaimed.unwrap_or(0).max(0) as u64)
    }

    // ------------------------------------------------------------------ exec

    /// Run argv inside a container and collect its output.
    ///
    /// Note the argv vector: the Electron backend built a shell string here
    /// (`docker exec <id> systemctl start <service>`), which let any service
    /// name containing `;` execute arbitrary commands. There is no shell in
    /// this path at all.
    pub async fn exec_capture(&self, id: &str, argv: Vec<String>) -> AppResult<String> {
        let exec = self
            .docker
            .create_exec(
                id,
                bollard::exec::CreateExecOptions {
                    cmd: Some(argv),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await?;

        let started = self
            .docker
            .start_exec(&exec.id, None::<bollard::exec::StartExecOptions>)
            .await?;

        match started {
            bollard::exec::StartExecResults::Attached { mut output, .. } => {
                let mut buf = String::new();
                while let Some(chunk) = output.next().await {
                    buf.push_str(&chunk?.to_string());
                }
                Ok(buf)
            }
            bollard::exec::StartExecResults::Detached => Ok(String::new()),
        }
    }

    /// Attach an interactive TTY to a new process inside the container.
    ///
    /// `tty: true` matters for more than echo: it stops the daemon multiplexing
    /// the response, so what comes back is the raw byte stream a terminal
    /// emulator expects rather than 8-byte-framed stdout/stderr records.
    pub async fn exec_start(
        &self,
        id: &str,
        argv: Vec<String>,
        cols: u16,
        rows: u16,
    ) -> AppResult<ExecAttach> {
        if argv.is_empty() || argv.iter().all(|a| a.trim().is_empty()) {
            return Err(AppError::Invalid("exec requires a command".into()));
        }

        let exec = self
            .docker
            .create_exec(
                id,
                bollard::exec::CreateExecOptions {
                    cmd: Some(argv),
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    // Without a TERM the shell assumes a dumb terminal and
                    // emits no colour or cursor addressing at all.
                    env: Some(vec!["TERM=xterm-256color".to_string()]),
                    ..Default::default()
                },
            )
            .await?;

        let started = self
            .docker
            .start_exec(
                &exec.id,
                Some(bollard::exec::StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: None,
                }),
            )
            .await?;

        match started {
            bollard::exec::StartExecResults::Attached { output, input } => {
                // The size can only be set once the process exists, so this is a
                // second round trip rather than a create-time option. Failure is
                // survivable — an 80x24 terminal is worse than a correct one but
                // better than no session — so it must not abort the attach.
                if let Err(e) = self.exec_resize(&exec.id, cols, rows).await {
                    eprintln!(
                        "cleat: initial exec resize failed ({e}); continuing at default size"
                    );
                }

                let stream = output.map(|item| match item {
                    Ok(out) => Ok(out.into_bytes()),
                    Err(e) => Err(AppError::Engine(e)),
                });

                Ok(ExecAttach {
                    exec_id: exec.id,
                    output: Box::pin(stream),
                    stdin: input,
                })
            }
            // Only returned for detach:true, which we never ask for.
            bollard::exec::StartExecResults::Detached => Err(AppError::Other(
                "runtime started the exec process without attaching a terminal".into(),
            )),
        }
    }

    pub async fn exec_resize(&self, exec_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        self.docker
            .resize_exec(
                exec_id,
                bollard::exec::ResizeExecOptions {
                    height: rows.max(1),
                    width: cols.max(1),
                },
            )
            .await?;
        Ok(())
    }

    /// Find an interactive shell that actually exists in this image.
    ///
    /// `/bin/bash` is absent from Alpine and most slim images, so defaulting to
    /// it makes the terminal fail on a large share of real containers. The probe
    /// is a fixed string with nothing interpolated into it.
    pub async fn detect_shell(&self, id: &str) -> AppResult<String> {
        const PROBE: &str = "command -v bash || command -v zsh || command -v ash || command -v sh";
        let out = self
            .exec_capture(id, vec!["/bin/sh".into(), "-c".into(), PROBE.into()])
            .await
            .unwrap_or_default();

        let found = out
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with('/') && !l.contains(char::is_whitespace));

        // A container with no `/bin/sh` at all (distroless) reaches here; hand
        // back the conventional path so the caller surfaces the daemon's own
        // "no such file" rather than a guess of our own.
        Ok(found.unwrap_or("/bin/sh").to_string())
    }
}

// ---------------------------------------------------------------- helpers

/// Collapse "no healthcheck" onto a single value across runtimes.
///
/// Docker omits `State.Health` entirely when a container has no healthcheck;
/// Podman includes it with an empty status. Without this, Podman containers
/// render an empty health badge in the UI, because the frontend keys off
/// `health !== "none"`.
fn normalize_health(raw: Option<String>) -> String {
    match raw {
        Some(s) if !s.trim().is_empty() => s,
        _ => "none".to_string(),
    }
}

/// Parse `"8080:80"` / `"8080:80/udp"` into (host_port, "80/tcp").
fn parse_port_spec(spec: &str) -> AppResult<(String, String)> {
    let (host, rest) = spec
        .split_once(':')
        .ok_or_else(|| AppError::Invalid(format!("port '{spec}' must be HOST:CONTAINER")))?;
    let (container, proto) = match rest.split_once('/') {
        Some((c, p)) => (c, p),
        None => (rest, "tcp"),
    };
    if host.parse::<u16>().is_err() || container.parse::<u16>().is_err() {
        return Err(AppError::Invalid(format!("port '{spec}' is not numeric")));
    }
    Ok((host.to_string(), format!("{container}/{proto}")))
}

/// Collapse a raw stats frame into the handful of numbers the UI charts.
fn reduce_stats(id: &str, raw: bollard::models::ContainerStatsResponse) -> Stats {
    let cpu = raw.cpu_stats.as_ref();
    let precpu = raw.precpu_stats.as_ref();

    let cpu_total = cpu
        .and_then(|c| c.cpu_usage.as_ref())
        .and_then(|u| u.total_usage)
        .unwrap_or(0);
    let pre_total = precpu
        .and_then(|c| c.cpu_usage.as_ref())
        .and_then(|u| u.total_usage)
        .unwrap_or(0);
    let sys = cpu.and_then(|c| c.system_cpu_usage).unwrap_or(0);
    let pre_sys = precpu.and_then(|c| c.system_cpu_usage).unwrap_or(0);
    let online = cpu
        .and_then(|c| c.online_cpus)
        .filter(|n| *n > 0)
        .unwrap_or(1) as f64;

    // saturating_sub: the daemon can report a lower reading after a counter
    // reset, and an underflow here would render as a nonsense spike.
    let cpu_delta = cpu_total.saturating_sub(pre_total) as f64;
    let sys_delta = sys.saturating_sub(pre_sys) as f64;
    let cpu_percent = if sys_delta > 0.0 && cpu_delta > 0.0 {
        (cpu_delta / sys_delta) * online * 100.0
    } else {
        0.0
    };

    let mem = raw.memory_stats.as_ref();
    let mem_usage_raw = mem.and_then(|m| m.usage).unwrap_or(0);
    // Docker's own CLI subtracts the page cache; without this the number reads
    // far higher than `docker stats` shows for the same container.
    let cache = mem
        .and_then(|m| m.stats.as_ref())
        .and_then(|s| s.get("inactive_file").or_else(|| s.get("cache")).copied())
        .unwrap_or(0);
    let mem_usage = mem_usage_raw.saturating_sub(cache);
    let mem_limit = mem.and_then(|m| m.limit).unwrap_or(0);
    let mem_percent = if mem_limit > 0 {
        (mem_usage as f64 / mem_limit as f64) * 100.0
    } else {
        0.0
    };

    let (rx, tx) = raw
        .networks
        .as_ref()
        .map(|nets| {
            nets.values().fold((0u64, 0u64), |(r, t), n| {
                (r + n.rx_bytes.unwrap_or(0), t + n.tx_bytes.unwrap_or(0))
            })
        })
        .unwrap_or((0, 0));

    let (block_read, block_write) = raw
        .blkio_stats
        .as_ref()
        .and_then(|b| b.io_service_bytes_recursive.as_ref())
        .map(|entries| {
            entries.iter().fold((0u64, 0u64), |(r, w), e| {
                match e.op.as_deref().map(|o| o.to_ascii_lowercase()).as_deref() {
                    Some("read") => (r + e.value.unwrap_or(0), w),
                    Some("write") => (r, w + e.value.unwrap_or(0)),
                    _ => (r, w),
                }
            })
        })
        .unwrap_or((0, 0));

    Stats {
        id: id.to_string(),
        name: raw
            .name
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string(),
        cpu_percent: (cpu_percent * 100.0).round() / 100.0,
        memory_usage: mem_usage,
        memory_limit: mem_limit,
        memory_percent: (mem_percent * 100.0).round() / 100.0,
        network_rx: rx,
        network_tx: tx,
        block_read,
        block_write,
        pids: raw.pids_stats.and_then(|p| p.current).unwrap_or(0),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    }
}

/// Parse `systemctl list-units --type=service --no-pager --plain` output.
pub fn parse_systemctl_units(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.contains(".service") {
                return None;
            }
            let mut cols = line.split_whitespace();
            // Columns: UNIT LOAD ACTIVE SUB DESCRIPTION. A bulleted first
            // column marks degraded units, so skip it when present.
            let first = cols.next()?;
            let unit = if first == "*" || first == "\u{25cf}" {
                cols.next()?
            } else {
                first
            };
            if !unit.ends_with(".service") {
                return None;
            }
            let _load = cols.next();
            let active = cols.next().unwrap_or("unknown");
            Some((unit.to_string(), active.to_string()))
        })
        .collect()
}

/// Ensure a service name can only ever be a unit name, never an argument or
/// a second command.
pub fn validate_service_name(name: &str) -> AppResult<()> {
    if name.is_empty() || name.len() > 128 {
        return Err(AppError::Invalid(
            "service name has an invalid length".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '\\'))
    {
        return Err(AppError::Invalid(format!(
            "service name '{name}' contains disallowed characters"
        )));
    }
    if name.starts_with('-') {
        return Err(AppError::Invalid(
            "service name may not start with '-'".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_port_spec() {
        let (host, container) = parse_port_spec("8080:80").unwrap();
        assert_eq!(host, "8080");
        assert_eq!(container, "80/tcp");
    }

    #[test]
    fn parses_udp_port_spec() {
        let (_, container) = parse_port_spec("5353:53/udp").unwrap();
        assert_eq!(container, "53/udp");
    }

    #[test]
    fn rejects_malformed_port_specs() {
        assert!(parse_port_spec("8080").is_err());
        assert!(parse_port_spec("abc:80").is_err());
        assert!(parse_port_spec("8080:xyz").is_err());
    }

    #[test]
    fn accepts_ordinary_service_names() {
        for name in ["nginx", "nginx.service", "getty@tty1.service", "my_svc-1"] {
            assert!(validate_service_name(name).is_ok(), "rejected {name}");
        }
    }

    /// The Electron backend interpolated this value into a shell string, so
    /// each of these executed a second command. They must not even reach argv.
    #[test]
    fn rejects_shell_metacharacters_in_service_names() {
        for name in [
            "nginx; rm -rf /",
            "nginx && whoami",
            "nginx|tee /etc/passwd",
            "$(id)",
            "`id`",
            "nginx\nreboot",
            "nginx > /etc/cron.d/x",
            "--force",
            "",
        ] {
            assert!(
                validate_service_name(name).is_err(),
                "accepted dangerous name: {name:?}"
            );
        }
    }

    #[test]
    fn parses_systemctl_units_output() {
        let out = "\
ssh.service    loaded active   running OpenBSD Secure Shell server
cron.service   loaded active   running Regular background program
foo.service    loaded inactive dead    Some stopped thing
";
        let units = parse_systemctl_units(out);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0], ("ssh.service".into(), "active".into()));
        assert_eq!(units[2], ("foo.service".into(), "inactive".into()));
    }

    #[test]
    fn skips_bullet_column_for_degraded_units() {
        let out = "\u{25cf} broken.service loaded failed failed Broken thing\n";
        let units = parse_systemctl_units(out);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].0, "broken.service");
        assert_eq!(units[0].1, "failed");
    }

    #[test]
    fn ignores_non_service_lines() {
        let out = "LISTED UNITS\nsome-header-line\nssh.service loaded active running x\n";
        assert_eq!(parse_systemctl_units(out).len(), 1);
    }

    fn cpu(total: u64, system: u64, online: u32) -> bollard::models::ContainerCpuStats {
        bollard::models::ContainerCpuStats {
            cpu_usage: Some(bollard::models::ContainerCpuUsage {
                total_usage: Some(total),
                ..Default::default()
            }),
            system_cpu_usage: Some(system),
            online_cpus: Some(online),
            ..Default::default()
        }
    }

    #[test]
    fn computes_cpu_percent_from_deltas() {
        let raw = bollard::models::ContainerStatsResponse {
            cpu_stats: Some(cpu(200, 2000, 4)),
            precpu_stats: Some(cpu(100, 1000, 4)),
            ..Default::default()
        };
        // (100/1000) * 4 cores * 100 = 40%
        assert_eq!(reduce_stats("abc", raw).cpu_percent, 40.0);
    }

    /// A counter reset would underflow and render as an absurd spike.
    #[test]
    fn clamps_cpu_percent_on_counter_reset() {
        let raw = bollard::models::ContainerStatsResponse {
            cpu_stats: Some(cpu(50, 500, 2)),
            precpu_stats: Some(cpu(100, 1000, 2)),
            ..Default::default()
        };
        assert_eq!(reduce_stats("abc", raw).cpu_percent, 0.0);
    }

    #[test]
    fn subtracts_page_cache_from_memory_usage() {
        let mut stats = std::collections::HashMap::new();
        stats.insert("inactive_file".to_string(), 300u64);
        let raw = bollard::models::ContainerStatsResponse {
            memory_stats: Some(bollard::models::ContainerMemoryStats {
                usage: Some(1000),
                limit: Some(2000),
                stats: Some(stats),
                ..Default::default()
            }),
            ..Default::default()
        };
        let s = reduce_stats("abc", raw);
        assert_eq!(s.memory_usage, 700);
        assert_eq!(s.memory_percent, 35.0);
    }

    #[test]
    fn tolerates_stats_with_no_cpu_or_memory() {
        let s = reduce_stats("abc", Default::default());
        assert_eq!(s.cpu_percent, 0.0);
        assert_eq!(s.memory_usage, 0);
        assert_eq!(s.memory_percent, 0.0);
    }
}
