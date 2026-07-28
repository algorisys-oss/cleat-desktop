//! Shared integration suite, run against every runtime.
//!
//! Each `suite::*` function takes a `&dyn ContainerRuntime`, so Docker and
//! Podman are held to identical assertions. That is the point: the trait's
//! value is that callers cannot tell the backends apart, and only running the
//! same suite against both actually demonstrates it.
//!
//! Every test skips cleanly when the runtime is unreachable. Resources are
//! prefixed `cleat-test-` and removed in the same function, including on the
//! failure paths.

#![allow(dead_code)] // each test binary uses a different subset

use cleat_lib::model::CreateContainerRequest;
use cleat_lib::runtime::ContainerRuntime;
use futures_util::StreamExt;

/// Distinct names per runtime, so a Docker run and a Podman run can overlap.
pub struct Names {
    pub container: String,
    pub volume: String,
    pub network: String,
}

impl Names {
    pub fn for_runtime(tag: &str) -> Self {
        Self {
            container: format!("cleat-test-{tag}-container"),
            volume: format!("cleat-test-{tag}-volume"),
            network: format!("cleat-test-{tag}-network"),
        }
    }
}

/// Smallest image we are willing to pull when the runtime has none.
pub const FALLBACK_IMAGE: &str = "alpine:latest";

/// Find a usable local image, pulling a small one only if the store is empty.
///
/// Prefers whatever is already present so the suite normally touches no
/// network; returns None (test skips) if the store is empty *and* the pull
/// fails, which is what happens on an offline machine.
pub async fn ensure_image(rt: &dyn ContainerRuntime) -> Option<String> {
    if let Ok(images) = rt.list_images().await {
        for preferred in ["alpine", "busybox", "nginx"] {
            if let Some(tag) = images.iter().find_map(|i| {
                i.repo_tags
                    .iter()
                    // `contains`, not `starts_with`: Podman fully-qualifies tags
                    // as `docker.io/library/alpine:latest`, so a prefix match
                    // silently fell through to "any image" — which could be one
                    // another test was about to delete.
                    .find(|t| t.contains(preferred) && !t.contains("<none>"))
                    .cloned()
            }) {
                return Some(tag);
            }
        }
        if let Some(any) = images
            .iter()
            .find_map(|i| i.repo_tags.iter().find(|t| !t.contains("<none>")).cloned())
        {
            return Some(any);
        }
    }

    eprintln!("no local images; pulling {FALLBACK_IMAGE}");
    match pull_to_completion(rt, FALLBACK_IMAGE).await {
        Ok(_) => Some(FALLBACK_IMAGE.to_string()),
        Err(e) => {
            eprintln!("skipping: no local image and pull failed ({e})");
            None
        }
    }
}

/// Drive a pull to completion, returning the progress events observed.
pub async fn pull_to_completion(
    rt: &dyn ContainerRuntime,
    image: &str,
) -> Result<Vec<cleat_lib::model::PullProgress>, String> {
    let mut stream = rt.pull_image(image).await.map_err(|e| e.message())?;
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(p) => {
                if let Some(err) = &p.error {
                    return Err(err.clone());
                }
                events.push(p);
            }
            Err(e) => return Err(e.message()),
        }
    }
    Ok(events)
}

pub mod suite {
    use super::*;

    pub async fn system_summary(rt: &dyn ContainerRuntime) {
        let s = rt.system_summary().await.expect("system summary");
        assert!(!s.version.is_empty(), "daemon version should be populated");
        assert!(!s.api_version.is_empty(), "api version should be populated");
        assert!(s.cpus > 0, "host should report at least one CPU");
        assert_eq!(s.runtime, rt.kind(), "summary must report its own runtime");
    }

    pub async fn list_containers(rt: &dyn ContainerRuntime) {
        // Two calls cannot be atomic, and the rest of the suite is creating and
        // destroying containers on other threads throughout. Comparing *counts*
        // sampled `all`-then-`running` therefore fails whenever a container
        // starts between them — nothing to do with the `all` flag being wrong.
        //
        // Sampling `running` first makes concurrent arrivals harmless: they can
        // only ever add to the later, wider set. The remaining hazard is a
        // container removed between the two calls, which is what the retry
        // absorbs.
        let mut missing = Vec::new();
        for attempt in 0..2 {
            let running = rt.list_containers(false).await.expect("list running");
            let all = rt.list_containers(true).await.expect("list all");

            let all_ids: std::collections::HashSet<_> = all.iter().map(|c| c.id.as_str()).collect();
            missing = running
                .iter()
                .filter(|c| !all_ids.contains(c.id.as_str()))
                .map(|c| format!("{} ({})", c.name, c.id))
                .collect();

            if missing.is_empty() {
                break;
            }
            eprintln!("list_containers: retrying after churn on attempt {attempt}");
        }
        assert!(
            missing.is_empty(),
            "running containers absent from the full list: {missing:?}"
        );

        let all = rt.list_containers(true).await.expect("list all");
        for c in &all {
            assert!(!c.id.is_empty(), "container id must be populated");
            assert!(!c.name.is_empty(), "container name must be populated");
            assert!(
                !c.name.starts_with('/'),
                "leading slash not stripped: {}",
                c.name
            );
            assert_ne!(
                c.state, "unknown",
                "state failed to deserialise for {}",
                c.name
            );
        }
    }

    pub async fn list_images(rt: &dyn ContainerRuntime) {
        let images = rt.list_images().await.expect("list images");
        for i in &images {
            assert!(!i.id.is_empty());
            assert!(i.size >= 0, "size should never be negative");
        }
    }

    pub async fn list_networks(rt: &dyn ContainerRuntime) {
        let networks = rt.list_networks().await.expect("list networks");
        assert!(
            !networks.is_empty(),
            "every runtime ships at least one default network"
        );
        for n in &networks {
            assert!(!n.id.is_empty(), "network id must be populated");
            assert!(!n.name.is_empty(), "network name must be populated");
            assert!(!n.driver.is_empty(), "network driver must be populated");
        }
    }

    pub async fn volume_roundtrip(rt: &dyn ContainerRuntime, names: &Names) {
        let name = &names.volume;
        let _ = rt.remove_volume(name, true).await; // leftover from an aborted run

        let created = rt.create_volume(name, None).await.expect("create volume");
        assert_eq!(&created.name, name);
        assert!(!created.mountpoint.is_empty(), "mountpoint should be set");

        let listed = rt.list_volumes().await.expect("list volumes");
        assert!(
            listed.iter().any(|v| &v.name == name),
            "created volume missing from list"
        );

        let inspected = rt.inspect_volume(name).await.expect("inspect volume");
        assert_eq!(
            inspected.get("Name").and_then(|v| v.as_str()),
            Some(name.as_str())
        );

        rt.remove_volume(name, false).await.expect("remove volume");
        let after = rt.list_volumes().await.expect("list after remove");
        assert!(
            !after.iter().any(|v| &v.name == name),
            "volume survived removal"
        );
    }

    pub async fn network_roundtrip(rt: &dyn ContainerRuntime, names: &Names) {
        let name = &names.network;
        if let Ok(existing) = rt.list_networks().await {
            if let Some(old) = existing.iter().find(|n| &n.name == name) {
                let _ = rt.remove_network(&old.id).await;
            }
        }

        let id = rt
            .create_network(name, "bridge", false)
            .await
            .expect("create network");
        assert!(!id.is_empty());

        let inspected = rt.inspect_network(&id).await.expect("inspect network");
        assert_eq!(
            inspected.get("Name").and_then(|v| v.as_str()),
            Some(name.as_str())
        );

        rt.remove_network(&id).await.expect("remove network");
    }

    /// Create → inspect → start → logs → stop → remove.
    pub async fn container_lifecycle(rt: &dyn ContainerRuntime, names: &Names) {
        let Some(image) = ensure_image(rt).await else {
            return;
        };
        let name = &names.container;
        let _ = rt.remove_container(name, true, true).await;

        let req = CreateContainerRequest {
            image: image.clone(),
            name: Some(name.clone()),
            env: vec!["CDLITE_TEST=1".to_string()],
            ports: vec![],
            volumes: vec![],
            network: None,
            command: vec!["sleep".into(), "30".into()],
            auto_remove: false,
            restart_policy: None,
        };

        let id = match rt.create_container(req).await {
            Ok(id) => id,
            Err(e) => {
                eprintln!("skipping: could not create from {image} ({})", e.message());
                return;
            }
        };

        let result = async {
            let listed = rt.list_containers(true).await?;
            let found = listed
                .iter()
                .find(|c| c.id == id)
                .expect("created container should appear in the list");
            assert_eq!(&found.name, name);

            let inspected = rt.inspect_container(&id).await?;
            assert!(
                inspected.get("Config").is_some(),
                "inspect should include Config"
            );

            // No healthcheck defined: must be "none", not an error.
            assert_eq!(rt.container_health(&id).await?, "none");

            rt.start_container(&id).await?;

            let running = rt.list_containers(false).await?;
            assert!(
                running.iter().any(|c| c.id == id),
                "started container should appear in the running list"
            );

            let _logs = rt.container_logs(&id, 50, false).await?;

            let _ = rt.stop_container(&id).await;
            Ok::<(), Box<dyn std::error::Error>>(())
        }
        .await;

        rt.remove_container(&id, true, true)
            .await
            .expect("cleanup: remove container");

        let after = rt.list_containers(true).await.expect("list after removal");
        assert!(
            !after.iter().any(|c| c.id == id),
            "container survived removal"
        );

        result.expect("lifecycle assertions");
    }

    /// Stats must stream real samples with a computable CPU percentage.
    ///
    /// This is the path that needs `one_shot: false`; without it the daemon
    /// omits `precpu_stats` and CPU% silently reads 0 forever.
    pub async fn stats_stream(rt: &dyn ContainerRuntime, names: &Names) {
        let Some(image) = ensure_image(rt).await else {
            return;
        };
        let name = format!("{}-stats", names.container);
        let _ = rt.remove_container(&name, true, true).await;

        let req = CreateContainerRequest {
            image,
            name: Some(name.clone()),
            env: vec![],
            ports: vec![],
            volumes: vec![],
            network: None,
            // Busy loop so there is CPU to measure. Exact argv: the whole
            // shell script must arrive as one argument to `-c`.
            command: vec!["sh".into(), "-c".into(), "while true; do :; done".into()],
            auto_remove: false,
            restart_policy: None,
        };

        let Ok(id) = rt.create_container(req).await else {
            eprintln!("skipping stats: could not create container");
            return;
        };

        let result = async {
            rt.start_container(&id).await?;
            let mut stream = rt.stream_stats(&id).await?;

            let mut samples = Vec::new();
            for _ in 0..3 {
                match tokio::time::timeout(std::time::Duration::from_secs(15), stream.next()).await
                {
                    Ok(Some(Ok(s))) => samples.push(s),
                    Ok(Some(Err(e))) => return Err(format!("stats error: {}", e.message()).into()),
                    Ok(None) => break,
                    Err(_) => return Err("timed out waiting for a stats sample".into()),
                }
            }

            assert!(!samples.is_empty(), "no stats samples arrived");
            for s in &samples {
                assert!(
                    s.cpu_percent >= 0.0 && s.cpu_percent.is_finite(),
                    "nonsensical cpu percent: {}",
                    s.cpu_percent
                );
                assert!(s.timestamp > 0, "sample should be timestamped");
            }
            // The second sample onward has a previous frame to diff against.
            if samples.len() > 1 {
                assert!(
                    samples[1..].iter().any(|s| s.cpu_percent > 0.0),
                    "a busy-looping container reported 0% CPU across all samples \
                     - precpu_stats is probably missing"
                );
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        }
        .await;

        let _ = rt.remove_container(&id, true, true).await;
        result.expect("stats assertions");
    }

    /// Log follow must deliver the container's output and then terminate.
    pub async fn log_follow(rt: &dyn ContainerRuntime, names: &Names) {
        let Some(image) = ensure_image(rt).await else {
            return;
        };
        let name = format!("{}-logs", names.container);
        let _ = rt.remove_container(&name, true, true).await;

        const MARKER: &str = "cleat-log-marker";
        let req = CreateContainerRequest {
            image,
            name: Some(name.clone()),
            env: vec![],
            ports: vec![],
            volumes: vec![],
            network: None,
            command: vec!["echo".into(), MARKER.into()],
            auto_remove: false,
            restart_policy: None,
        };

        let Ok(id) = rt.create_container(req).await else {
            eprintln!("skipping log follow: could not create container");
            return;
        };

        let result = async {
            let mut stream = rt.follow_logs(&id, 100).await?;
            rt.start_container(&id).await?;

            let mut seen = String::new();
            let deadline = std::time::Duration::from_secs(20);
            loop {
                match tokio::time::timeout(deadline, stream.next()).await {
                    Ok(Some(Ok(line))) => {
                        seen.push_str(&line.message);
                        seen.push('\n');
                        if seen.contains(MARKER) {
                            break;
                        }
                    }
                    Ok(Some(Err(e))) => return Err(format!("log error: {}", e.message()).into()),
                    // Stream ended: the container exited. Fall back to a
                    // one-shot read, since a fast `echo` can finish before the
                    // follow stream attaches.
                    Ok(None) => {
                        seen = rt.container_logs(&id, 100, false).await?;
                        break;
                    }
                    Err(_) => return Err("timed out waiting for log output".into()),
                }
            }

            assert!(
                seen.contains(MARKER),
                "log marker never appeared; got: {seen:?}"
            );
            Ok::<(), Box<dyn std::error::Error>>(())
        }
        .await;

        let _ = rt.remove_container(&id, true, true).await;
        result.expect("log follow assertions");
    }

    /// Errors must carry a kind, not collapse into one generic failure.
    pub async fn error_kinds(rt: &dyn ContainerRuntime) {
        let err = rt
            .inspect_container("cleat-definitely-does-not-exist")
            .await
            .expect_err("inspecting a missing container should fail");
        assert_eq!(
            err.kind(),
            "not_found",
            "got kind={} msg={}",
            err.kind(),
            err.message()
        );
    }

    /// Injection attempts must be rejected before reaching the daemon.
    pub async fn rejects_injection(rt: &dyn ContainerRuntime) {
        let err = rt
            .control_container_service("cleat-nonexistent", "nginx; rm -rf /", "start")
            .await
            .expect_err("shell metacharacter in service name must be rejected");
        assert_eq!(err.kind(), "invalid");

        let err = rt
            .control_container_service("cleat-nonexistent", "nginx", "start; reboot")
            .await
            .expect_err("unknown action must be rejected");
        assert_eq!(err.kind(), "invalid");

        let err = rt
            .control_container_service("cleat-nonexistent", "--force", "start")
            .await
            .expect_err("a name that is really a flag must be rejected");
        assert_eq!(err.kind(), "invalid");
    }

    /// Spawn a long-lived container so an interactive session has something to
    /// attach to. Returns None (test skips) when the runtime can't oblige.
    async fn long_running(rt: &dyn ContainerRuntime, name: &str) -> Option<String> {
        let image = ensure_image(rt).await?;
        let _ = rt.remove_container(name, true, true).await;

        let req = CreateContainerRequest {
            image,
            name: Some(name.to_string()),
            env: vec![],
            ports: vec![],
            volumes: vec![],
            network: None,
            command: vec!["sleep".into(), "120".into()],
            auto_remove: false,
            restart_policy: None,
        };

        let id = match rt.create_container(req).await {
            Ok(id) => id,
            Err(e) => {
                eprintln!(
                    "skipping exec test: could not create container ({})",
                    e.message()
                );
                return None;
            }
        };
        if let Err(e) = rt.start_container(&id).await {
            eprintln!(
                "skipping exec test: could not start container ({})",
                e.message()
            );
            let _ = rt.remove_container(&id, true, true).await;
            return None;
        }
        Some(id)
    }

    /// The round trip that matters: bytes written to stdin come back on the
    /// output stream, through a real TTY, on both runtimes.
    pub async fn exec_roundtrip(rt: &dyn ContainerRuntime, names: &Names) {
        let name = format!("{}-exec", names.container);
        let Some(id) = long_running(rt, &name).await else {
            return;
        };

        let result = async {
            let attach = rt
                .exec_start(&id, vec!["/bin/sh".into()], 80, 24)
                .await
                .map_err(|e| format!("exec_start: {}", e.message()))?;
            let cleat_lib::runtime::ExecAttach {
                exec_id,
                mut output,
                mut stdin,
            } = attach;

            assert!(!exec_id.is_empty(), "exec id must be populated for resize");

            const MARKER: &str = "cleat-exec-marker";
            {
                use tokio::io::AsyncWriteExt;
                stdin
                    .write_all(format!("echo {MARKER}\n").as_bytes())
                    .await
                    .map_err(|e| format!("write to exec stdin: {e}"))?;
                stdin.flush().await.map_err(|e| format!("flush: {e}"))?;
            }

            let mut seen = Vec::new();
            let deadline = std::time::Duration::from_secs(20);
            loop {
                match tokio::time::timeout(deadline, output.next()).await {
                    Ok(Some(Ok(chunk))) => {
                        seen.extend_from_slice(&chunk);
                        // Match on bytes: TTY output is not line-oriented and
                        // carries escape sequences around the payload.
                        if String::from_utf8_lossy(&seen).contains(MARKER) {
                            break;
                        }
                    }
                    Ok(Some(Err(e))) => return Err(format!("exec stream: {}", e.message())),
                    Ok(None) => return Err("exec stream ended before the marker".into()),
                    Err(_) => {
                        return Err(format!(
                            "timed out; saw {:?}",
                            String::from_utf8_lossy(&seen)
                        ))
                    }
                }
            }

            // A TTY echoes what was typed, which is the other half of proving
            // the session is interactive rather than a one-shot capture.
            let text = String::from_utf8_lossy(&seen).to_string();
            assert!(
                text.matches(MARKER).count() >= 1,
                "expected the marker in TTY output; got {text:?}"
            );
            Ok::<(), String>(())
        }
        .await;

        let _ = rt.remove_container(&id, true, true).await;
        result.expect("exec round trip");
    }

    /// Resize is the likeliest place the two runtimes diverge: it is a separate
    /// endpoint addressed by exec id, and Podman serves it from its
    /// compatibility layer. Pinning it on both is the whole point of the shared
    /// suite.
    pub async fn exec_resize(rt: &dyn ContainerRuntime, names: &Names) {
        let name = format!("{}-resize", names.container);
        let Some(id) = long_running(rt, &name).await else {
            return;
        };

        let result = async {
            let attach = rt
                .exec_start(&id, vec!["/bin/sh".into()], 80, 24)
                .await
                .map_err(|e| format!("exec_start: {}", e.message()))?;

            rt.exec_resize(&attach.exec_id, 120, 40)
                .await
                .map_err(|e| format!("resize to 120x40: {}", e.message()))?;

            // Degenerate sizes are clamped rather than sent to the daemon.
            rt.exec_resize(&attach.exec_id, 0, 0)
                .await
                .map_err(|e| format!("clamped resize: {}", e.message()))?;

            Ok::<(), String>(())
        }
        .await;

        let _ = rt.remove_container(&id, true, true).await;
        result.expect("exec resize");
    }

    /// The shell probe must name a path that exists in the image, not assume
    /// bash.
    pub async fn detect_shell(rt: &dyn ContainerRuntime, names: &Names) {
        let name = format!("{}-shell", names.container);
        let Some(id) = long_running(rt, &name).await else {
            return;
        };

        let result = async {
            let shell = rt
                .detect_shell(&id)
                .await
                .map_err(|e| format!("detect_shell: {}", e.message()))?;
            assert!(
                shell.starts_with('/'),
                "shell must be an absolute path, got {shell:?}"
            );

            // Whatever it named must actually be executable in this image.
            let attach = rt
                .exec_start(&id, vec![shell.clone()], 80, 24)
                .await
                .map_err(|e| format!("probed shell {shell} is not runnable: {}", e.message()))?;
            assert!(!attach.exec_id.is_empty());
            Ok::<(), String>(())
        }
        .await;

        let _ = rt.remove_container(&id, true, true).await;
        result.expect("detect shell");
    }

    /// An empty command must be refused here rather than turning into an
    /// opaque daemon error.
    pub async fn rejects_empty_exec(rt: &dyn ContainerRuntime) {
        let err = rt
            .exec_start("cleat-nonexistent", vec![], 80, 24)
            .await
            .expect_err("empty argv must be rejected");
        assert_eq!(err.kind(), "invalid");

        let err = rt
            .exec_start("cleat-nonexistent", vec!["   ".into()], 80, 24)
            .await
            .expect_err("whitespace-only argv must be rejected");
        assert_eq!(err.kind(), "invalid");
    }

    pub async fn rejects_bad_compose_dir(rt: &dyn ContainerRuntime) {
        let err = rt
            .compose_services("/cleat/no/such/directory")
            .await
            .expect_err("nonexistent project dir should be rejected");
        assert_eq!(err.kind(), "invalid");
    }
}
