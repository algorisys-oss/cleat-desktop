//! Shared integration suite, run against every runtime.
//!
//! Each `suite::*` function takes a `&dyn ContainerRuntime`, so Docker and
//! Podman are held to identical assertions. That is the point: the trait's
//! value is that callers cannot tell the backends apart, and only running the
//! same suite against both actually demonstrates it. The two `audit_*`
//! functions take an owned `Box<dyn ContainerRuntime>` instead, because the
//! decorator under test wraps a runtime rather than borrowing one.
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
/// Does `tag` name the repository `repo`, ignoring registry and version?
///
/// Substring matching was wrong in a way that only showed up on CI: `"alpine"`
/// is a substring of `"nginx:alpine"`, so asking for the smallest image could
/// hand back a 20 MB nginx. Compare the repository component instead, allowing
/// for the registry prefix Podman adds (`docker.io/library/alpine:latest`).
fn repo_matches(tag: &str, repo: &str) -> bool {
    if tag.contains("<none>") {
        return false;
    }
    // Strip the tag/digest, being careful that a registry may carry a port.
    let name = match tag.rsplit_once(':') {
        Some((left, right)) if !right.contains('/') => left,
        _ => tag,
    };
    let last = name.rsplit('/').next().unwrap_or(name);
    last == repo
}

#[cfg(test)]
mod repo_matches_tests {
    use super::repo_matches;

    #[test]
    fn matches_plain_and_qualified_names() {
        assert!(repo_matches("alpine:latest", "alpine"));
        assert!(repo_matches("docker.io/library/alpine:latest", "alpine"));
        assert!(repo_matches("localhost/alpine:latest", "alpine"));
        assert!(repo_matches("alpine", "alpine"));
    }

    /// The CI failure: "alpine" is a substring of "nginx:alpine", so the old
    /// check handed back nginx when asked for the smallest image.
    #[test]
    fn does_not_match_a_tag_that_merely_contains_the_name() {
        assert!(!repo_matches("nginx:alpine", "alpine"));
        assert!(!repo_matches("docker.io/library/nginx:alpine", "alpine"));
        assert!(!repo_matches("alpine-extras:latest", "alpine"));
        assert!(!repo_matches("myalpine:latest", "alpine"));
    }

    #[test]
    fn ignores_untagged_images() {
        assert!(!repo_matches("<none>:<none>", "alpine"));
    }

    /// A registry may carry a port, which must not be mistaken for the tag.
    #[test]
    fn tolerates_a_registry_port() {
        assert!(repo_matches("registry:5000/alpine:latest", "alpine"));
        assert!(repo_matches("registry:5000/alpine", "alpine"));
    }
}

pub async fn ensure_image(rt: &dyn ContainerRuntime) -> Option<String> {
    if let Ok(images) = rt.list_images().await {
        for preferred in ["alpine", "busybox", "nginx"] {
            if let Some(tag) = images.iter().find_map(|i| {
                i.repo_tags
                    .iter()
                    .find(|t| repo_matches(t, preferred))
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
    use cleat_lib::model::OpKind;
    use cleat_lib::runtime::audit::{ActivityLog, AuditRuntime};
    use std::sync::Arc;

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

    /// A public image still pulls once credentials are attached.
    ///
    /// Pulls now send whatever login the runtime has for the registry, so on a
    /// machine that *is* logged in to Docker Hub every public pull carries a
    /// credential it did not carry before. An expired or malformed one turns a
    /// working anonymous pull into a 401, which is a regression the resolver's
    /// own unit tests cannot see — they never reach a registry.
    ///
    /// Uses a tiny official image, and removes it only if this test introduced
    /// it, so a developer's existing images survive.
    pub async fn pull_public_image_with_ambient_credentials(rt: &dyn ContainerRuntime) {
        const REFERENCE: &str = "hello-world:latest";
        const REPO: &str = "hello-world";

        // Compared with `repo_matches`, not equality: Podman reports fully
        // qualified names (`docker.io/library/hello-world:latest`) where Docker
        // reports `hello-world:latest`. An equality check here passes on Docker
        // and fails on Podman, which is precisely the runtime-specific
        // assumption the shared suite exists to prevent.
        let had_it = |images: &[cleat_lib::model::Image]| {
            images
                .iter()
                .any(|i| i.repo_tags.iter().any(|t| repo_matches(t, REPO)))
        };
        let before = rt.list_images().await.expect("list images before pull");
        let preexisting = had_it(&before);

        let mut stream = rt.pull_image(REFERENCE).await.expect("start pull");
        let mut failure: Option<String> = None;
        while let Some(event) = stream.next().await {
            let progress = event.expect("pull stream item");
            if let Some(error) = progress.error {
                failure = Some(error);
            }
        }
        assert!(
            failure.is_none(),
            "pulling a public image failed with credentials attached: {}. \
             An ambient registry login should never make a public pull fail.",
            failure.unwrap_or_default()
        );

        let after = rt.list_images().await.expect("list images after pull");
        assert!(had_it(&after), "{REFERENCE} absent after a successful pull");

        if !preexisting {
            // Same reason as `had_it`: match the repository, not the literal
            // reference, or cleanup silently finds nothing on Podman and the
            // test leaves an image behind.
            let id = after
                .iter()
                .find(|i| i.repo_tags.iter().any(|t| repo_matches(t, REPO)))
                .map(|i| i.id.clone())
                .expect("just-pulled image");
            let _ = rt.remove_image(&id, false).await;
        }
    }

    /// Tagging points a second reference at an existing image.
    ///
    /// This is the step that makes a push addressable, so getting it wrong
    /// means every push targets the wrong repository. Uses a `localhost:5000`
    /// name: it exercises the registry-port case in `split_reference`, and it
    /// names a registry that cannot be reached by accident.
    ///
    /// Push itself has no live test. Verifying it would mean publishing to a
    /// real registry under someone's account, which a test suite has no
    /// business doing.
    pub async fn tag_image(rt: &dyn ContainerRuntime) {
        let Some(source) = super::ensure_image(rt).await else {
            eprintln!("skipping tag_image: no image available");
            return;
        };
        const TARGET: &str = "localhost:5000/cleat-test-tag:v1";

        rt.tag_image(&source, TARGET).await.expect("tag image");

        // Everything below identifies the image by id rather than by name.
        // Names are not runtime-neutral — Podman qualifies them
        // (`docker.io/library/alpine:latest`) and `ensure_image` may return the
        // unqualified string it pulled with — so a name comparison here passes
        // on one runtime and fails on the other. Ids are stable on both.
        let images = rt.list_images().await.expect("list images after tag");
        let tagged = images
            .iter()
            .find(|i| i.repo_tags.iter().any(|t| t.ends_with("cleat-test-tag:v1")))
            .unwrap_or_else(|| panic!("new tag absent from the image list"));
        let image_id = tagged.id.clone();

        // Removing the tag, not the image: other tags on the same layers, and
        // the image this test borrowed, must survive.
        rt.remove_image(TARGET, false)
            .await
            .expect("remove the test tag");

        let after = rt.list_images().await.expect("list images after untag");
        assert!(
            !after
                .iter()
                .any(|i| i.repo_tags.iter().any(|t| t.ends_with("cleat-test-tag:v1"))),
            "test tag survived removal"
        );
        assert!(
            after.iter().any(|i| i.id == image_id),
            "removing the test tag deleted the borrowed image {source} ({image_id})"
        );
    }

    /// An empty or whitespace-only target is refused before it reaches the
    /// daemon, whose own error for this is unhelpful.
    pub async fn tag_image_rejects_a_malformed_target(rt: &dyn ContainerRuntime) {
        let Some(source) = super::ensure_image(rt).await else {
            eprintln!("skipping tag_image_rejects: no image available");
            return;
        };
        for bad in ["", "   ", "has a space:v1"] {
            let err = rt.tag_image(&source, bad).await.expect_err(bad);
            assert!(
                matches!(err, cleat_lib::error::AppError::Invalid(_)),
                "{bad:?} gave {err:?}, expected Invalid"
            );
        }
    }

    /// Manifest generation against real inspect output.
    ///
    /// The unit tests for this feed it hand-written JSON, which proves the
    /// logic and nothing about the shape. Docker and Podman each report
    /// inspect slightly differently — Podman qualifies image names, the two
    /// disagree on which keys are present when empty — so this runs the real
    /// thing through and requires the result to parse as YAML.
    pub async fn generate_kubernetes_manifest(rt: &dyn ContainerRuntime, names: &Names) {
        use cleat_lib::k8s::generate;

        // Its own name, not `names.container`: the suite runs concurrently
        // against one daemon and `container_lifecycle` already owns that one,
        // so sharing it is a 409 whenever the two overlap.
        let name = format!("{}-k8sgen", names.container);

        let Some(image) = super::ensure_image(rt).await else {
            eprintln!("skipping generate: no image available");
            return;
        };

        let id = rt
            .create_container(cleat_lib::model::CreateContainerRequest {
                image,
                name: Some(name.clone()),
                env: vec!["APP_MODE=production".into(), "DB_PASSWORD=hunter2".into()],
                ports: vec![],
                volumes: vec![],
                network: None,
                command: vec!["sh".into(), "-c".into(), "sleep 300".into()],
                auto_remove: false,
                restart_policy: None,
            })
            .await
            .expect("create container");

        let generated = async {
            let inspect = rt.inspect_container(&id).await.expect("inspect");
            generate::from_inspect(&inspect, &generate::Options::default()).expect("generate")
        }
        .await;

        // Always clean up, including when an assertion below would panic.
        let cleanup = rt.remove_container(&id, true, false).await;

        assert!(
            generated.yaml.contains("kind: Deployment"),
            "no Deployment in:\n{}",
            generated.yaml
        );
        // The name came from the daemon, so this exercises the RFC 1123
        // rewrite against a real container name rather than a fixture.
        assert!(
            generated.yaml.contains(&format!(
                "name: {}",
                generate::sanitize_name(&names.container)
            )),
            "expected the sanitized container name in:\n{}",
            generated.yaml
        );

        let docs: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(&generated.yaml)
            .map(|d| serde::Deserialize::deserialize(d).expect("each document parses"))
            .collect();
        assert!(!docs.is_empty(), "generated nothing parseable");

        // A password in the environment must raise the warning, or the feature
        // silently writes secrets into files people commit.
        assert!(
            generated.warnings.iter().any(|w| w.kind == "secret-in-env"),
            "no secret warning for DB_PASSWORD: {:?}",
            generated.warnings
        );

        cleanup.expect("remove container");
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

    /// A freshly created volume must be reported with a size.
    ///
    /// The listing endpoint never carries usage on either runtime, so this is
    /// the claim the Size column rests on, and the two runtimes answer the
    /// disk-usage endpoint in two different shapes. A live check is the only
    /// thing that catches a daemon that stops filling either one in.
    pub async fn volume_usage_reports_sizes(rt: &dyn ContainerRuntime, names: &Names) {
        let name = format!("{}-usage", names.volume);
        let _ = rt.remove_volume(&name, true).await; // leftover from an aborted run
        rt.create_volume(&name, None).await.expect("create volume");

        let usage = rt.volume_usage().await;
        let _ = rt.remove_volume(&name, true).await;

        let usage = usage.expect("volume usage");
        // An empty volume is 0 bytes, not absent: absent means the daemon
        // declined to size it, which the UI then has to render as unknown.
        let size = usage
            .get(&name)
            .unwrap_or_else(|| panic!("{name} missing from disk usage: {usage:?}"));
        assert_eq!(*size, 0, "a new volume holds nothing");
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

    /// The activity log must record real operations, with timing and outcome.
    ///
    /// This is the claim the panel makes to the user — that what they see is
    /// everything Cleat did — so it is worth pinning against a live daemon
    /// rather than a mock.
    ///
    /// Takes an owned runtime because the decorator wraps rather than borrows,
    /// which is also why this is the one suite function that does not take
    /// `&dyn ContainerRuntime`.
    pub async fn audit_records_operations(rt: Box<dyn ContainerRuntime>, names: &Names) {
        let log = Arc::new(ActivityLog::new());
        let audited = AuditRuntime::new(rt, log.clone());

        assert!(log.snapshot().is_empty(), "log should start empty");

        audited.list_images().await.expect("list images");

        let after_read = log.snapshot();
        assert_eq!(after_read.len(), 1, "one operation, one entry");
        let entry = &after_read[0];
        assert_eq!(entry.op, "list_images");
        assert_eq!(
            entry.kind,
            OpKind::Read,
            "listing must not count as a change"
        );
        // Captured off the wire rather than authored next to the call. Matched
        // by prefix on purpose: bollard appends default query parameters here
        // and the exact set is its business, not a regression when it changes.
        assert_eq!(entry.requests.len(), 1, "one request: {:?}", entry.requests);
        assert!(
            entry.requests[0].starts_with("GET /images/json"),
            "should be the real request line, got {:?}",
            entry.requests[0]
        );
        assert!(
            entry.error.is_none(),
            "successful call must record no error"
        );

        // A failure must be recorded, not swallowed — the log is most useful
        // precisely when something went wrong.
        let _ = audited
            .inspect_container("cleat-definitely-does-not-exist")
            .await;
        let after_failure = log.snapshot();
        assert_eq!(after_failure.len(), 2);
        assert_eq!(
            after_failure[0].op, "inspect_container",
            "snapshot must be newest first"
        );
        assert!(
            after_failure[0].error.is_some(),
            "the failure should have been recorded with its message"
        );
        // Capture is not conditional on success: a request that came back 404
        // was still a request Cleat made, and hiding it would defeat the panel.
        assert!(
            after_failure[0]
                .requests
                .iter()
                .any(|r| r.contains("cleat-definitely-does-not-exist")),
            "the failed request should still be recorded: {:?}",
            after_failure[0].requests
        );

        // Query parameters must survive into the log. This is the concrete
        // thing the old authored strings got wrong — `list_images` claimed a
        // bare `GET /images/json` while bollard was sending four parameters —
        // and it is what a reader checking "did it list *my* stopped containers
        // too" needs.
        log.clear();
        audited
            .list_containers(true)
            .await
            .expect("list containers");
        let listed = &log.snapshot()[0];
        assert!(
            listed.requests.iter().any(|r| r.contains("all=true")),
            "the argument that changes the result must be visible: {:?}",
            listed.requests
        );

        // An operation that issues several requests must show all of them. This
        // is the case an authored string could not represent honestly, and the
        // reason the field is a list.
        log.clear();
        audited.system_summary().await.expect("system summary");
        let summary = &log.snapshot()[0];
        assert_eq!(
            summary.requests.len(),
            2,
            "version + info are two calls: {:?}",
            summary.requests
        );
        assert!(
            summary.requests.iter().any(|r| r.ends_with("/version"))
                && summary.requests.iter().any(|r| r.ends_with("/info")),
            "both endpoints should appear: {:?}",
            summary.requests
        );

        // Writes must be distinguishable, since the panel filters on that.
        let volume = format!("{}-audit", names.volume);
        let _ = audited.remove_volume(&volume, true).await;
        audited
            .create_volume(&volume, None)
            .await
            .expect("create volume");
        let created = log
            .snapshot()
            .into_iter()
            .find(|e| e.op == "create_volume")
            .expect("create_volume should be in the log");
        assert_eq!(created.kind, OpKind::Write);
        assert!(
            created
                .args
                .iter()
                .any(|(k, v)| k == "name" && *v == volume),
            "arguments should be recorded: {:?}",
            created.args
        );
        let _ = audited.remove_volume(&volume, true).await;

        log.clear();
        assert!(log.snapshot().is_empty(), "clear should empty the log");
    }

    /// Starting a shell must record every request it makes.
    ///
    /// There are three, and the count is the point: the hand-written string
    /// this replaced advertised two, having forgotten that the TTY size can
    /// only be set once the process exists and costs another round trip. Nobody
    /// was going to notice that by reading the call site.
    ///
    /// Exec is also the path the capture hook is least obviously correct on: it
    /// does not go through bollard's ordinary response handling but through
    /// `process_upgraded`, which hijacks the connection. It still builds its
    /// request through `build_request`, which is where the hook lives — but
    /// that is a fact about bollard's internals, and an upgrade could change it
    /// without changing anything Cleat compiles against. If that happened the
    /// panel would quietly stop reporting the most privileged operation in the
    /// app.
    pub async fn audit_records_every_exec_request(rt: Box<dyn ContainerRuntime>, names: &Names) {
        let log = Arc::new(ActivityLog::new());
        let audited = AuditRuntime::new(rt, log.clone());

        let name = format!("{}-audit-exec", names.container);
        let Some(id) = long_running(&audited, &name).await else {
            return;
        };

        log.clear();
        let attached = audited
            .exec_start(&id, vec!["/bin/sh".into()], 80, 24)
            .await;
        let entry = log
            .snapshot()
            .into_iter()
            .find(|e| e.op == "exec_start")
            .expect("exec_start should be in the log");

        let _ = audited.remove_container(&id, true, true).await;
        attached.expect("exec_start");

        assert_eq!(
            entry.requests.len(),
            3,
            "create the session, start it, size the terminal: {:?}",
            entry.requests
        );
        assert!(
            entry.requests[0].ends_with("/exec")
                && entry.requests[1].ends_with("/start")
                && entry.requests[2].contains("/resize"),
            "all three should be recorded, in the order they went out: {:?}",
            entry.requests
        );
    }
}
