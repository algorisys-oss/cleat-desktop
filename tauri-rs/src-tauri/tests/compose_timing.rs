//! Compose streaming behaviour against a real project.
//!
//! Regression for a report that `Down` appeared to hang: it never did, but
//! Docker waits a 10-second SIGTERM grace period per stubborn container, and
//! the old blocking call showed nothing for the duration. These assert that
//! output arrives *while* the command runs, and that it terminates.

use futures_util::StreamExt;
use std::time::Instant;
use cleat_lib::runtime::docker::DockerRuntime;
use cleat_lib::runtime::{ComposeAction, ContainerRuntime};

/// Project dir provided by the harness below.
fn fixture() -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("cleat-compose-fixture");
    std::fs::create_dir_all(&dir).ok()?;
    // A container that ignores SIGTERM, so `down` must wait the full grace
    // period — the exact case that looked like a hang.
    std::fs::write(
        dir.join("compose.yaml"),
        "services:\n  stubborn:\n    image: nginx:alpine\n    \
         entrypoint: [\"sh\", \"-c\", \"trap '' TERM INT; while true; do sleep 1; done\"]\n",
    )
    .ok()?;
    Some(dir)
}

#[tokio::test]
async fn compose_down_streams_output_and_terminates() {
    let Ok(rt) = DockerRuntime::connect().await else {
        eprintln!("skipping: no docker");
        return;
    };
    let Some(dir) = fixture() else { return };
    let dir = dir.to_string_lossy().to_string();

    // Bring it up first (also proves `up` streams).
    let mut up = rt
        .compose_exec(&dir, ComposeAction::Up, None)
        .await
        .expect("compose up");
    let mut up_lines = Vec::new();
    while let Some(item) = up.next().await {
        up_lines.push(item.expect("up stream error"));
    }
    assert!(!up_lines.is_empty(), "compose up produced no output");

    // Now the case that looked hung.
    let started = Instant::now();
    let mut first_line_at = None;
    let mut down = rt
        .compose_exec(&dir, ComposeAction::Down, None)
        .await
        .expect("compose down");

    let mut lines = Vec::new();
    while let Some(item) = down.next().await {
        if first_line_at.is_none() {
            first_line_at = Some(started.elapsed());
        }
        lines.push(item.expect("down stream error"));
    }
    let total = started.elapsed();

    assert!(!lines.is_empty(), "compose down produced no output");

    // The point of the fix: feedback arrives long before the command finishes,
    // so the UI is never silent for the whole grace period.
    let first = first_line_at.expect("no lines observed");
    assert!(
        first < total,
        "first line at {first:?} but command took {total:?} - output was not streamed"
    );
    assert!(
        total.as_secs() < 120,
        "compose down took {total:?}; it should terminate"
    );
    println!("first output at {first:?}, completed in {total:?}, {} lines", lines.len());
}

/// A compose file the runtime would never find by name still works when the
/// user points at the file itself.
///
/// The directory picker greys files out, so this path only exists because a
/// file picker was added alongside it; `stack.yml` is not one of the four names
/// compose searches for, and the same directory must therefore fail when named
/// as a directory. That contrast is the whole test.
#[tokio::test]
async fn a_named_compose_file_is_used_directly() {
    let Ok(rt) = DockerRuntime::connect().await else {
        eprintln!("skipping: no docker");
        return;
    };
    let dir = std::env::temp_dir().join("cleat-compose-named-fixture");
    std::fs::create_dir_all(&dir).expect("fixture dir");
    std::fs::write(
        dir.join("stack.yml"),
        "services:\n  idle:\n    image: nginx:alpine\n",
    )
    .expect("fixture file");

    rt.compose_services(&dir.join("stack.yml").to_string_lossy())
        .await
        .expect("a named compose file should be usable");

    let err = rt
        .compose_services(&dir.to_string_lossy())
        .await
        .expect_err("the directory alone holds no file compose would find");
    assert_eq!(err.kind(), "not_found");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `down` applies to the whole project; naming a service must be rejected.
#[tokio::test]
async fn compose_down_rejects_a_service_argument() {
    let Ok(rt) = DockerRuntime::connect().await else {
        eprintln!("skipping: no docker");
        return;
    };
    let Some(dir) = fixture() else { return };
    // LineStream isn't Debug, so expect_err() won't compile here.
    let err = match rt
        .compose_exec(&dir.to_string_lossy(), ComposeAction::Down, Some("stubborn"))
        .await
    {
        Ok(_) => panic!("down with a service argument should have been rejected"),
        Err(e) => e,
    };
    assert_eq!(err.kind(), "invalid");
}

/// An injected service name must not reach argv.
#[tokio::test]
async fn compose_exec_rejects_injected_service_name() {
    let Ok(rt) = DockerRuntime::connect().await else {
        eprintln!("skipping: no docker");
        return;
    };
    let Some(dir) = fixture() else { return };
    let err = match rt
        .compose_exec(
            &dir.to_string_lossy(),
            ComposeAction::Restart,
            Some("web; rm -rf /"),
        )
        .await
    {
        Ok(_) => panic!("a service name with shell metacharacters should have been rejected"),
        Err(e) => e,
    };
    assert_eq!(err.kind(), "invalid");
}
