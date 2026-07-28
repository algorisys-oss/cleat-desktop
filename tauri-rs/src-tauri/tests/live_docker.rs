//! Docker integration tests against a real daemon.
//!
//! The assertions live in `common::suite` and are shared with
//! `live_podman.rs`, so both backends are held to the same contract.
//! Skips cleanly when Docker is unreachable.

mod common;

use common::{suite, Names};
use cleat_lib::runtime::docker::DockerRuntime;
use cleat_lib::runtime::ContainerRuntime;

async fn rt() -> Option<(DockerRuntime, Names)> {
    match DockerRuntime::connect().await {
        Ok(rt) => Some((rt, Names::for_runtime("docker"))),
        Err(e) => {
            eprintln!("skipping: docker unavailable ({})", e.message());
            None
        }
    }
}

#[tokio::test]
async fn reports_system_summary() {
    let Some((rt, _)) = rt().await else { return };
    suite::system_summary(&rt).await;
}

#[tokio::test]
async fn lists_containers_with_mapped_fields() {
    let Some((rt, _)) = rt().await else { return };
    suite::list_containers(&rt).await;
}

#[tokio::test]
async fn lists_images_with_sizes() {
    let Some((rt, _)) = rt().await else { return };
    suite::list_images(&rt).await;
}

#[tokio::test]
async fn lists_networks() {
    let Some((rt, _)) = rt().await else { return };
    suite::list_networks(&rt).await;
}

#[tokio::test]
async fn volume_roundtrip() {
    let Some((rt, names)) = rt().await else { return };
    suite::volume_roundtrip(&rt, &names).await;
}

#[tokio::test]
async fn network_roundtrip() {
    let Some((rt, names)) = rt().await else { return };
    suite::network_roundtrip(&rt, &names).await;
}

#[tokio::test]
async fn container_lifecycle() {
    let Some((rt, names)) = rt().await else { return };
    suite::container_lifecycle(&rt, &names).await;
}

#[tokio::test]
async fn stats_stream_reports_cpu() {
    let Some((rt, names)) = rt().await else { return };
    suite::stats_stream(&rt, &names).await;
}

#[tokio::test]
async fn log_follow_delivers_output() {
    let Some((rt, names)) = rt().await else { return };
    suite::log_follow(&rt, &names).await;
}

#[tokio::test]
async fn maps_error_kinds() {
    let Some((rt, _)) = rt().await else { return };
    suite::error_kinds(&rt).await;
}

#[tokio::test]
async fn rejects_injection_in_service_control() {
    let Some((rt, _)) = rt().await else { return };
    suite::rejects_injection(&rt).await;
}

#[tokio::test]
async fn rejects_bad_compose_project_dir() {
    let Some((rt, _)) = rt().await else { return };
    suite::rejects_bad_compose_dir(&rt).await;
}

/// Docker's compose front-end must be the v2 subcommand, not the EOL v1 binary.
#[tokio::test]
async fn uses_compose_v2() {
    let Some((rt, _)) = rt().await else { return };
    assert_eq!(
        rt.compose_argv(),
        vec!["docker".to_string(), "compose".to_string()]
    );
}

/// Podman must never resolve to the Docker socket.
///
/// Lives here, not in `live_podman.rs`, on purpose: this is the case where
/// Podman is *absent*, so the Podman suite would skip and prove nothing.
/// bollard's `connect_with_podman_defaults()` falls back to
/// `/var/run/docker.sock`, which made Podman report itself available with
/// Docker's version number on a Docker-only machine.
#[tokio::test]
async fn podman_does_not_masquerade_as_docker() {
    use cleat_lib::model::RuntimeKind;
    use cleat_lib::runtime::detect_runtimes;

    let runtimes = detect_runtimes().await;
    let podman = runtimes
        .iter()
        .find(|r| r.kind == RuntimeKind::Podman)
        .expect("podman must appear in the runtime list");
    let docker = runtimes
        .iter()
        .find(|r| r.kind == RuntimeKind::Docker)
        .expect("docker must appear in the runtime list");

    let podman_socket = std::path::Path::new(&format!(
        "{}/podman/podman.sock",
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/0".into())
    ))
    .exists()
        || std::path::Path::new("/run/podman/podman.sock").exists();

    if !podman_socket {
        assert!(
            !podman.available,
            "podman reported available with no podman socket present (detail: {:?})",
            podman.detail
        );
    }

    if podman.available && docker.available {
        assert_ne!(
            podman.version, docker.version,
            "podman and docker reported identical versions - podman is resolving to the docker socket"
        );
    }
}
