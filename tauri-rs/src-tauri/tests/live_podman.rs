//! Podman integration tests against a real daemon.
//!
//! Runs the *same* `common::suite` assertions as `live_docker.rs`. That is the
//! actual proof the runtime abstraction holds: identical expectations, two
//! backends, no per-runtime special-casing in the assertions.
//!
//! Skips cleanly when Podman is unreachable. Enable its API socket with:
//!   systemctl --user start podman.socket

mod common;

use cleat_lib::model::RuntimeKind;
use cleat_lib::runtime::podman::PodmanRuntime;
use cleat_lib::runtime::ContainerRuntime;
use common::{suite, Names};

async fn rt() -> Option<(PodmanRuntime, Names)> {
    match PodmanRuntime::connect().await {
        Ok(rt) => Some((rt, Names::for_runtime("podman"))),
        Err(e) => {
            eprintln!("skipping: podman unavailable ({})", e.message());
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
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::volume_roundtrip(&rt, &names).await;
}

#[tokio::test]
async fn network_roundtrip() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::network_roundtrip(&rt, &names).await;
}

#[tokio::test]
async fn container_lifecycle() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::container_lifecycle(&rt, &names).await;
}

#[tokio::test]
async fn stats_stream_reports_cpu() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::stats_stream(&rt, &names).await;
}

#[tokio::test]
async fn log_follow_delivers_output() {
    let Some((rt, names)) = rt().await else {
        return;
    };
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
async fn exec_roundtrip() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::exec_roundtrip(&rt, &names).await;
}

/// Podman serves `/exec/{id}/resize` from its Docker compatibility layer, so
/// this is the assertion most likely to catch a real divergence.
#[tokio::test]
async fn exec_resize() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::exec_resize(&rt, &names).await;
}

#[tokio::test]
async fn detects_a_usable_shell() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::detect_shell(&rt, &names).await;
}

#[tokio::test]
async fn rejects_empty_exec_command() {
    let Some((rt, _)) = rt().await else { return };
    suite::rejects_empty_exec(&rt).await;
}

#[tokio::test]
async fn rejects_bad_compose_project_dir() {
    let Some((rt, _)) = rt().await else { return };
    suite::rejects_bad_compose_dir(&rt).await;
}

// --- Podman-specific behaviour -------------------------------------------

/// The runtime must identify as Podman, not inherit Docker's identity.
#[tokio::test]
async fn identifies_as_podman() {
    let Some((rt, _)) = rt().await else { return };
    assert_eq!(rt.kind(), RuntimeKind::Podman);
    let summary = rt.system_summary().await.expect("system summary");
    assert_eq!(summary.runtime, RuntimeKind::Podman);
}

/// netavark has no overlay driver; we reject it with a reason rather than
/// letting the daemon return an opaque error.
#[tokio::test]
async fn rejects_overlay_network_driver() {
    let Some((rt, _)) = rt().await else { return };
    let err = rt
        .create_network("cleat-test-podman-overlay", "overlay", false)
        .await
        .expect_err("overlay should be rejected on Podman");
    assert_eq!(err.kind(), "invalid");
    assert!(
        err.message().to_lowercase().contains("overlay"),
        "message should name the unsupported driver: {}",
        err.message()
    );
}

/// Podman's compose front-end, whichever provider is installed.
#[tokio::test]
async fn uses_a_podman_compose_frontend() {
    let Some((rt, _)) = rt().await else { return };
    let argv = rt.compose_argv();
    assert!(
        argv[0] == "podman" || argv[0] == "podman-compose",
        "unexpected compose front-end: {argv:?}"
    );
}

/// Docker and Podman must be distinct daemons, not the same socket twice.
///
/// Regression for bollard's `connect_with_podman_defaults()`, which falls back
/// to `/var/run/docker.sock` and made Podman report Docker's version.
#[tokio::test]
async fn is_not_the_docker_daemon() {
    let Some((podman, _)) = rt().await else {
        return;
    };
    let Ok(docker) = cleat_lib::runtime::docker::DockerRuntime::connect().await else {
        eprintln!("skipping: docker unavailable, nothing to compare against");
        return;
    };

    let p = podman.system_summary().await.expect("podman summary");
    let d = docker.system_summary().await.expect("docker summary");

    assert_ne!(
        p.version, d.version,
        "podman and docker reported the same version - podman is resolving to the docker socket"
    );
    assert_eq!(p.runtime, RuntimeKind::Podman);
    assert_eq!(d.runtime, RuntimeKind::Docker);
}

/// Copy a real image from Docker into Podman through the Engine API.
///
/// The two runtimes keep entirely separate image stores, so this is the only
/// way "the same image on both" is achievable. Exercises export → import
/// streaming end to end; no CLI, no temp file.
#[tokio::test]
async fn copies_an_image_from_docker() {
    use futures_util::StreamExt;

    let Some((podman, _)) = rt().await else {
        return;
    };
    let Ok(docker) = cleat_lib::runtime::docker::DockerRuntime::connect().await else {
        eprintln!("skipping: docker unavailable, nothing to copy from");
        return;
    };

    // Ask for a known-small image rather than "whichever is smallest right
    // now". The old approach sampled a list that other tests mutate in
    // parallel, so which image got copied varied per machine — on CI it picked
    // a 20 MB nginx, and the failure was impossible to reproduce locally.
    let Some(source) = common::ensure_image(&docker).await else {
        eprintln!("skipping: docker has no tagged images");
        return;
    };

    // If the image is already in Podman, another test may be using it; leave it
    // alone rather than deleting it out from under a parallel test.
    let pre_existing = podman
        .list_images()
        .await
        .map(|imgs| {
            imgs.iter()
                .any(|i| i.repo_tags.iter().any(|t| t.ends_with(&source)))
        })
        .unwrap_or(false);
    if pre_existing {
        eprintln!("skipping: {source} already present in podman");
        return;
    }

    let export = docker
        .export_image(&source)
        .await
        .expect("export from docker");
    let mut import = podman
        .import_image(Box::pin(export))
        .await
        .expect("import to podman");

    let mut statuses = Vec::new();
    while let Some(item) = import.next().await {
        statuses.push(item.expect("import stream error"));
    }

    // Podman requalifies references as it loads them — `alpine:latest` comes
    // back as `localhost/alpine:latest` — so requiring the source name to
    // reappear verbatim asserts something the operation never promised.
    // Verify what the runtime *said* it loaded is actually there.
    let loaded: Vec<String> = statuses
        .iter()
        .filter_map(|s| {
            s.split_once("Loaded image:")
                .map(|(_, r)| r.trim().to_string())
        })
        .filter(|r| !r.is_empty())
        .collect();

    let after = podman.list_images().await.expect("list podman images");
    let tags: Vec<&str> = after
        .iter()
        .flat_map(|i| i.repo_tags.iter().map(String::as_str))
        .collect();

    // Fall back to the source name for runtimes that report the load
    // differently; the point is that *something* arrived under a name we can
    // tie back to the copy.
    let expected: Vec<String> = if loaded.is_empty() {
        vec![source.clone()]
    } else {
        loaded.clone()
    };
    let present = expected
        .iter()
        .any(|want| tags.iter().any(|t| t == want || t.ends_with(want.as_str())));

    // Clean up before asserting, so a failure cannot leave the image behind.
    for reference in expected.iter().chain(std::iter::once(&source)) {
        let _ = podman.remove_image(reference, true).await;
    }

    assert!(
        present,
        "copy from docker reported {statuses:?} but none of {expected:?} is in podman; \
         podman has: {tags:?}"
    );
}

/// Podman's client must be instrumented too.
///
/// The decorator is runtime-agnostic, but the capture hook is installed per
/// client — `docker.rs` and `podman.rs` each call `wire::instrument`, and a new
/// backend could forget to. Only running this against both proves it.
#[tokio::test]
async fn audit_records_operations() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::audit_records_operations(Box::new(rt), &names).await;
}

#[tokio::test]
async fn audit_records_every_exec_request() {
    let Some((rt, names)) = rt().await else {
        return;
    };
    suite::audit_records_every_exec_request(Box::new(rt), &names).await;
}
