//! Where credentials are read from, exercised against real files on disk.
//!
//! The pure resolution logic is unit-tested next to the code. What this covers
//! is the seam those tests cannot reach: whether `DOCKER_CONFIG` and
//! `REGISTRY_AUTH_FILE` are actually honoured, and whether Docker's and
//! Podman's config locations stay distinct. Getting a path wrong here makes the
//! whole feature silently do nothing — every pull just falls back to anonymous,
//! which looks exactly like "not logged in".
//!
//! These live in their own test binary because they mutate process environment.
//! Cargo runs each integration test file as its own process, so nothing else in
//! the suite can observe it — but the tests *within* this file share one
//! process, hence the mutex.

use cleat_lib::model::RuntimeKind;
use cleat_lib::runtime::credentials::{identity_for, list_logins};
use std::sync::Mutex;

/// The environment is process-wide, so these cannot overlap.
static ENV: Mutex<()> = Mutex::new(());

/// Run `f` with exactly `vars` set, then put the environment back.
///
/// `HOME` is always redirected into the scratch directory. Without that, a
/// developer machine with a real `~/.docker/config.json` fails these tests for
/// the wrong reason — an assertion about a temp file silently resolves against
/// the maintainer's own Docker Hub login. Found exactly that way.
fn with_env<T>(
    home: &std::path::Path,
    vars: &[(&str, &std::path::Path)],
    f: impl FnOnce() -> T,
) -> T {
    let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());

    let home_key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mut restore: Vec<(String, Option<std::ffi::OsString>)> =
        vec![(home_key.to_string(), std::env::var_os(home_key))];
    std::env::set_var(home_key, home);

    for (key, value) in vars {
        restore.push((key.to_string(), std::env::var_os(key)));
        std::env::set_var(key, value);
    }
    // Podman consults this before any file location; a real one in the ambient
    // environment would otherwise take precedence over the fixture.
    for key in ["REGISTRY_AUTH_FILE", "XDG_RUNTIME_DIR", "DOCKER_CONFIG"] {
        if !vars.iter().any(|(k, _)| *k == key) {
            restore.push((key.to_string(), std::env::var_os(key)));
            std::env::remove_var(key);
        }
    }

    let result = f();

    for (key, value) in restore {
        match value {
            Some(v) => std::env::set_var(&key, v),
            None => std::env::remove_var(&key),
        }
    }
    result
}

/// alice:s3cr3t
const ALICE: &str = "YWxpY2U6czNjcjN0";

fn write_config(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).expect("create config dir");
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write config");
    path
}

/// A scratch directory that cleans itself up, named so a leaked one is obvious.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("cleat-test-creds-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create temp dir");
        Self(base)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn docker_config_env_var_is_honoured() {
    let dir = TempDir::new("docker-env");
    let cfg = dir.path().join("cfg");
    write_config(
        &cfg,
        "config.json",
        &format!(r#"{{"auths":{{"ghcr.io":{{"auth":"{ALICE}"}}}}}}"#),
    );

    let id = with_env(dir.path(), &[("DOCKER_CONFIG", &cfg)], || {
        identity_for(RuntimeKind::Docker, "ghcr.io/owner/app:1")
    });

    assert_eq!(id.registry, "ghcr.io");
    assert_eq!(
        id.username.as_deref(),
        Some("alice"),
        "DOCKER_CONFIG was not read: {id:?}"
    );
    assert_eq!(id.source.as_deref(), Some("config.json"));
}

#[test]
fn podman_reads_its_own_auth_file_not_dockers() {
    let dir = TempDir::new("podman-split");
    let docker_cfg = dir.path().join("docker");
    let podman_cfg = dir.path().join("podman");

    // Two different users, so picking the wrong file is visible rather than
    // coincidentally right.
    write_config(
        &docker_cfg,
        "config.json",
        &format!(r#"{{"auths":{{"ghcr.io":{{"auth":"{ALICE}"}}}}}}"#),
    );
    let podman_file = write_config(
        &podman_cfg,
        "auth.json",
        // bob:hunter2
        r#"{"auths":{"ghcr.io":{"auth":"Ym9iOmh1bnRlcjI="}}}"#,
    );

    let (docker, podman) = with_env(
        dir.path(),
        &[
            ("DOCKER_CONFIG", &docker_cfg),
            ("REGISTRY_AUTH_FILE", &podman_file),
        ],
        || {
            (
                identity_for(RuntimeKind::Docker, "ghcr.io/owner/app"),
                identity_for(RuntimeKind::Podman, "ghcr.io/owner/app"),
            )
        },
    );

    assert_eq!(docker.username.as_deref(), Some("alice"), "{docker:?}");
    assert_eq!(podman.username.as_deref(), Some("bob"), "{podman:?}");
}

/// A config file that is not valid JSON must degrade to anonymous pulls, which
/// still work for public images, rather than surfacing an error into the UI.
#[test]
fn a_corrupt_config_falls_back_to_anonymous() {
    let dir = TempDir::new("corrupt");
    let cfg = dir.path().join("cfg");
    write_config(&cfg, "config.json", "{ this is not json");

    let (id, logins) = with_env(dir.path(), &[("DOCKER_CONFIG", &cfg)], || {
        (
            identity_for(RuntimeKind::Docker, "nginx"),
            list_logins(RuntimeKind::Docker),
        )
    });

    assert_eq!(id.registry, "docker.io");
    assert_eq!(id.source, None, "expected anonymous, got {id:?}");
    assert!(
        logins.is_empty(),
        "corrupt file produced logins: {logins:?}"
    );
}

#[test]
fn a_missing_config_is_not_an_error() {
    let dir = TempDir::new("missing");
    let cfg = dir.path().join("nowhere");

    let id = with_env(dir.path(), &[("DOCKER_CONFIG", &cfg)], || {
        identity_for(RuntimeKind::Docker, "ghcr.io/owner/app")
    });

    assert_eq!(id.source, None, "expected anonymous, got {id:?}");
}

/// The panel lists what is on disk, and a helper-only registry has to appear
/// even though its secret lives somewhere Cleat deliberately does not look.
#[test]
fn listing_reports_helper_backed_registries() {
    let dir = TempDir::new("helpers");
    let cfg = dir.path().join("cfg");
    write_config(
        &cfg,
        "config.json",
        r#"{"credHelpers":{"gcr.io":"gcloud"}}"#,
    );

    let logins = with_env(dir.path(), &[("DOCKER_CONFIG", &cfg)], || {
        list_logins(RuntimeKind::Docker)
    });

    let gcr = logins
        .iter()
        .find(|l| l.registry == "gcr.io")
        .unwrap_or_else(|| panic!("gcr.io missing from {logins:?}"));
    assert_eq!(gcr.source.as_deref(), Some("docker-credential-gcloud"));
    assert_eq!(gcr.username, None, "a helper's secret must not be read");
}

/// The fallback chain must not stop at the first *path*, only at the first file
/// that exists and parses — Podman's runtime location is frequently absent.
#[test]
fn podman_falls_through_to_the_persistent_location() {
    let dir = TempDir::new("podman-fallthrough");
    // $XDG_RUNTIME_DIR/containers/auth.json is deliberately not created.
    let runtime_dir = dir.path().join("run");
    write_config(
        &dir.path().join(".config").join("containers"),
        "auth.json",
        &format!(r#"{{"auths":{{"quay.io":{{"auth":"{ALICE}"}}}}}}"#),
    );

    let id = with_env(dir.path(), &[("XDG_RUNTIME_DIR", &runtime_dir)], || {
        identity_for(RuntimeKind::Podman, "quay.io/org/img")
    });

    assert_eq!(id.username.as_deref(), Some("alice"), "{id:?}");
    assert_eq!(id.source.as_deref(), Some("auth.json"));
}
