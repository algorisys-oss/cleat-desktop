//! Registry credentials, read from what the user has already logged into.
//!
//! # Why read rather than store
//!
//! Cleat deliberately does not have its own credential store. `docker login`
//! and `podman login` already wrote credentials somewhere the runtime trusts,
//! and duplicating them into a third location would mean a second copy of a
//! secret to leak, expire, and disagree with the first. So this module is
//! read-only: it resolves what is already on disk and hands it to the daemon
//! for the length of one request.
//!
//! # What is on disk
//!
//! Both runtimes use the same schema in different places:
//!
//! ```json
//! {
//!   "auths": { "https://index.docker.io/v1/": { "auth": "<base64 user:pass>" } },
//!   "credsStore": "desktop",
//!   "credHelpers": { "gcr.io": "gcloud" }
//! }
//! ```
//!
//! `auths` holds the secret inline. `credsStore` and `credHelpers` instead name
//! an external helper binary that holds it — which is the common case on a
//! desktop, because that is what keeps the secret in the OS keychain rather
//! than base64 in a dotfile.
//!
//! # Secrets and the activity log
//!
//! Credentials reach the daemon in the `X-Registry-Auth` header.
//! [`crate::runtime::wire`] records only method and path, never headers or
//! bodies, so nothing here can reach the activity log. That is a property of
//! the capture hook rather than of this module, and
//! `secrets_in_headers_are_not_recorded` in `wire.rs` holds it in place.

use crate::error::{AppError, AppResult};
use crate::model::RuntimeKind;
use base64::Engine as _;
use bollard::auth::DockerCredentials;
use std::collections::HashMap;
use std::path::PathBuf;

/// Docker Hub's key in `auths`, which is a v1 URL for historical reasons rather
/// than the hostname anyone would guess.
const DOCKER_HUB_KEY: &str = "https://index.docker.io/v1/";
/// What an image reference means when it names no registry.
const DOCKER_HUB_HOST: &str = "docker.io";

/// One registry the user is logged in to, without the secret.
///
/// Deliberately not `DockerCredentials`: this is what the UI is allowed to see,
/// and it must not be possible to render a password by accident.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RegistryLogin {
    pub registry: String,
    /// `None` when a helper owns the secret, since reading it would prompt for
    /// a keychain unlock, or when there is no login at all.
    pub username: Option<String>,
    /// Where the credential lives: the config file, or the helper that owns it.
    /// `None` means there is no credential and the pull will be anonymous.
    pub source: Option<String>,
}

/// The parsed config file, before any helper has been consulted.
#[derive(Debug, Default, serde::Deserialize)]
struct AuthConfig {
    #[serde(default)]
    auths: HashMap<String, AuthEntry>,
    #[serde(default, rename = "credsStore")]
    creds_store: Option<String>,
    #[serde(default, rename = "credHelpers")]
    cred_helpers: HashMap<String, String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct AuthEntry {
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default, rename = "identitytoken")]
    identity_token: Option<String>,
}

/// The registry an image reference points at.
///
/// The rule Docker uses, and the one that surprises people: the first path
/// segment is a registry only if it looks like a host — it contains a dot or a
/// colon, or it is exactly `localhost`. Everything else is a Hub namespace, so
/// `library/nginx` is Hub while `localhost:5000/nginx` is not.
pub fn registry_for(image: &str) -> String {
    let reference = image.trim().trim_start_matches('/');
    match reference.split_once('/') {
        Some((head, _)) if head == "localhost" || head.contains('.') || head.contains(':') => {
            head.to_string()
        }
        _ => DOCKER_HUB_HOST.to_string(),
    }
}

/// Config files to consult for `kind`, most specific first.
///
/// Docker and Podman keep separate login state, and a user logged in to a
/// private registry with one has not logged in with the other. Reading the
/// wrong file would silently pull with the wrong identity.
fn config_paths(kind: RuntimeKind) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    match kind {
        RuntimeKind::Docker => {
            if let Some(dir) = env_path("DOCKER_CONFIG") {
                paths.push(dir.join("config.json"));
            }
            if let Some(home) = home_dir() {
                paths.push(home.join(".docker").join("config.json"));
            }
        }
        RuntimeKind::Podman => {
            // Podman's own override, then the rootless runtime location it
            // writes by default, then the persistent one.
            if let Some(file) = env_path("REGISTRY_AUTH_FILE") {
                paths.push(file);
            }
            if let Some(dir) = env_path("XDG_RUNTIME_DIR") {
                paths.push(dir.join("containers").join("auth.json"));
            }
            if let Some(home) = home_dir() {
                paths.push(home.join(".config").join("containers").join("auth.json"));
                // Podman falls back to Docker's file, so a `docker login` is
                // honoured by `podman pull`. Mirroring that keeps Cleat's
                // behaviour the same as the CLI the user already knows.
                paths.push(home.join(".docker").join("config.json"));
            }
        }
    }
    paths
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    let key = "HOME";
    #[cfg(windows)]
    let key = "USERPROFILE";
    env_path(key)
}

/// Load the first config file that exists and parses.
///
/// A malformed file is skipped rather than fatal: a broken `config.json` should
/// degrade to anonymous pulls, which still work for public images, instead of
/// breaking the image list.
fn load_config(kind: RuntimeKind) -> Option<(AuthConfig, PathBuf)> {
    for path in config_paths(kind) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<AuthConfig>(&text) {
            Ok(config) => return Some((config, path)),
            Err(_) => continue,
        }
    }
    None
}

/// Keys under which `registry` might be filed in `auths`.
///
/// Written by several generations of CLI, so the same registry appears as a
/// bare host, an https URL, or — for Hub only — the v1 API URL.
fn lookup_keys(registry: &str) -> Vec<String> {
    let mut keys = vec![
        registry.to_string(),
        format!("https://{registry}"),
        format!("https://{registry}/"),
    ];
    if registry == DOCKER_HUB_HOST || registry == "index.docker.io" {
        keys.push(DOCKER_HUB_KEY.to_string());
        keys.push("index.docker.io".to_string());
    }
    keys
}

/// Split a decoded `user:password` pair.
///
/// Passwords may contain colons; usernames may not, so the split is on the
/// first one only.
fn split_auth(decoded: &str) -> Option<(String, String)> {
    decoded
        .split_once(':')
        .map(|(u, p)| (u.to_string(), p.to_string()))
}

fn decode_auth(auth: &str) -> Option<(String, String)> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(auth.trim())
        .ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    split_auth(&decoded)
}

/// Which helper owns `registry`, if any.
///
/// A per-registry entry in `credHelpers` wins over the global `credsStore`,
/// which is how a machine can keep most logins in the OS keychain while
/// delegating one cloud registry to its own tool.
fn helper_for<'a>(config: &'a AuthConfig, registry: &str) -> Option<&'a str> {
    for key in lookup_keys(registry) {
        if let Some(helper) = config.cred_helpers.get(&key) {
            return Some(helper.as_str());
        }
    }
    config.creds_store.as_deref()
}

/// What a credential helper prints on stdout.
#[derive(Debug, serde::Deserialize)]
struct HelperResponse {
    #[serde(rename = "Username")]
    username: Option<String>,
    #[serde(rename = "Secret")]
    secret: Option<String>,
}

/// The sentinel a helper returns instead of a username when the secret is an
/// identity token rather than a password.
const TOKEN_USERNAME: &str = "<token>";

/// Ask a credential helper for `registry`.
///
/// The helper protocol is argv plus stdin — `docker-credential-<name> get` with
/// the server on stdin — so there is no shell here and no user-supplied text
/// becomes an argument. The helper name comes from the user's own config file,
/// and is constrained to a plain identifier before it is spawned: a config
/// naming `../../evil` must not turn into a path.
async fn ask_helper(helper: &str, registry: &str) -> AppResult<HelperResponse> {
    if helper.is_empty()
        || !helper
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AppError::Invalid(format!(
            "credential helper name {helper:?} is not a plain identifier"
        )));
    }

    let binary = format!("docker-credential-{helper}");
    let mut child = tokio::process::Command::new(&binary)
        .arg("get")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| AppError::Other(format!("{binary} could not be run: {e}")))?;

    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Other(format!("{binary} has no stdin")))?;
        stdin
            .write_all(registry.as_bytes())
            .await
            .map_err(|e| AppError::Other(format!("writing to {binary}: {e}")))?;
        // Dropped here on purpose: the helper reads to EOF and will not answer
        // until stdin closes.
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Other(format!("waiting for {binary}: {e}")))?;

    if !output.status.success() {
        // Not an error worth surfacing: "no credentials for this registry" is
        // the ordinary answer for any registry the user has not logged in to.
        return Err(AppError::NotFound(format!(
            "{binary} has no credentials for {registry}"
        )));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| AppError::Other(format!("parsing {binary} output: {e}")))
}

/// Resolve credentials for whatever registry `image` lives in.
///
/// Returns `None` when the user is not logged in there, which is not an error —
/// public images pull anonymously and that is the common case.
pub async fn resolve(kind: RuntimeKind, image: &str) -> Option<DockerCredentials> {
    let registry = registry_for(image);
    let (config, _) = load_config(kind)?;

    // A helper owns the secret if one is configured, even when `auths` also has
    // an entry — the entry is usually a leftover stub with an empty `auth`.
    if let Some(helper) = helper_for(&config, &registry) {
        if let Ok(response) = ask_helper(helper, &registry).await {
            let secret = response.secret.unwrap_or_default();
            if !secret.is_empty() {
                return Some(match response.username.as_deref() {
                    Some(TOKEN_USERNAME) | None => DockerCredentials {
                        identitytoken: Some(secret),
                        serveraddress: Some(registry.clone()),
                        ..Default::default()
                    },
                    Some(user) => DockerCredentials {
                        username: Some(user.to_string()),
                        password: Some(secret),
                        serveraddress: Some(registry.clone()),
                        ..Default::default()
                    },
                });
            }
        }
    }

    for key in lookup_keys(&registry) {
        let Some(entry) = config.auths.get(&key) else {
            continue;
        };
        if let Some(token) = entry.identity_token.as_ref().filter(|t| !t.is_empty()) {
            return Some(DockerCredentials {
                identitytoken: Some(token.clone()),
                serveraddress: Some(registry.clone()),
                ..Default::default()
            });
        }
        if let Some((user, password)) = entry.auth.as_deref().and_then(decode_auth) {
            return Some(DockerCredentials {
                username: Some(user),
                password: Some(password),
                serveraddress: Some(registry.clone()),
                ..Default::default()
            });
        }
        if let (Some(user), Some(password)) = (&entry.username, &entry.password) {
            return Some(DockerCredentials {
                username: Some(user.clone()),
                password: Some(password.clone()),
                serveraddress: Some(registry.clone()),
                ..Default::default()
            });
        }
    }

    None
}

/// Every registry this runtime has a login for, without secrets.
///
/// Helpers are *not* consulted: asking one can prompt for a keychain unlock,
/// and a list rendered in a panel must not make the OS ask for a password. The
/// helper's name is reported as the source instead, which is the honest answer
/// — Cleat knows a credential exists there without having read it.
pub fn list_logins(kind: RuntimeKind) -> Vec<RegistryLogin> {
    match load_config(kind) {
        Some((config, path)) => logins_in(&config, &source_label(&path)),
        None => Vec::new(),
    }
}

/// How a config file is named in the UI: its basename, since the full path is
/// long and the basename already distinguishes Docker's from Podman's.
fn source_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string())
}

/// The pure half of [`list_logins`], so the shape can be tested without a
/// filesystem or a mutated environment.
fn logins_in(config: &AuthConfig, file: &str) -> Vec<RegistryLogin> {
    let mut logins: Vec<RegistryLogin> = Vec::new();

    for (key, entry) in &config.auths {
        if !is_registry_key(key) {
            continue;
        }
        let registry = normalise_key(key);
        let username = entry
            .username
            .clone()
            .or_else(|| entry.auth.as_deref().and_then(decode_auth).map(|(u, _)| u));
        logins.push(RegistryLogin {
            registry,
            username,
            source: Some(file.to_string()),
        });
    }

    for (key, helper) in &config.cred_helpers {
        let registry = normalise_key(key);
        if logins.iter().any(|l| l.registry == registry) {
            continue;
        }
        logins.push(RegistryLogin {
            registry,
            username: None,
            source: Some(format!("docker-credential-{helper}")),
        });
    }

    logins.sort_by(|a, b| a.registry.cmp(&b.registry));
    logins
}

/// What `image` would authenticate as, without unlocking anything.
///
/// Same restraint as [`list_logins`]: no helper is invoked, so a dialog can ask
/// this on every keystroke without the OS popping a keychain prompt. A
/// helper-owned registry therefore reports the helper as its source and no
/// username — Cleat knows a credential is there without having read it.
pub fn identity_for(kind: RuntimeKind, image: &str) -> RegistryLogin {
    let registry = registry_for(image);
    match load_config(kind) {
        Some((config, path)) => identity_in(&config, &source_label(&path), &registry),
        None => anonymous(registry),
    }
}

fn anonymous(registry: String) -> RegistryLogin {
    RegistryLogin {
        registry,
        username: None,
        source: None,
    }
}

/// The pure half of [`identity_for`].
fn identity_in(config: &AuthConfig, file: &str, registry: &str) -> RegistryLogin {
    if let Some(helper) = helper_for(config, registry) {
        return RegistryLogin {
            registry: registry.to_string(),
            username: None,
            source: Some(format!("docker-credential-{helper}")),
        };
    }

    for key in lookup_keys(registry) {
        let Some(entry) = config.auths.get(&key) else {
            continue;
        };
        let username = entry
            .username
            .clone()
            .or_else(|| entry.auth.as_deref().and_then(decode_auth).map(|(u, _)| u));
        // An entry with neither a decodable pair nor a token is a leftover stub
        // — `docker logout` leaves these behind — and claiming it as a login
        // would promise an authenticated pull that then 401s.
        let has_secret =
            username.is_some() || entry.identity_token.as_ref().is_some_and(|t| !t.is_empty());
        if !has_secret {
            continue;
        }
        return RegistryLogin {
            registry: registry.to_string(),
            username,
            source: Some(file.to_string()),
        };
    }

    anonymous(registry.to_string())
}

/// Is this `auths` key a registry, or one of Docker's private stashes?
///
/// A web or token login writes `https://index.docker.io/v1/access-token` and
/// `.../refresh-token` next to the real entry. They have the same shape as a
/// login and decode like one, so listing them unfiltered puts two rows in the
/// registries panel that are not registries and cannot be logged out of. A real
/// key is a host with an optional port and nothing else, so anything with a
/// path left after the scheme is stripped is not one.
fn is_registry_key(key: &str) -> bool {
    !normalise_key(key).contains('/')
}

/// Present an `auths` key as the hostname a person would recognise.
fn normalise_key(key: &str) -> String {
    if key == DOCKER_HUB_KEY || key == "index.docker.io" {
        return DOCKER_HUB_HOST.to_string();
    }
    key.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_names_are_docker_hub() {
        assert_eq!(registry_for("nginx"), "docker.io");
        assert_eq!(registry_for("nginx:alpine"), "docker.io");
        assert_eq!(registry_for("library/nginx"), "docker.io");
        assert_eq!(registry_for("rajesh/cleat:1.2.3"), "docker.io");
    }

    #[test]
    fn a_dotted_or_ported_first_segment_is_a_registry() {
        assert_eq!(registry_for("ghcr.io/owner/app:1"), "ghcr.io");
        assert_eq!(registry_for("quay.io/org/img"), "quay.io");
        assert_eq!(registry_for("localhost:5000/foo"), "localhost:5000");
        assert_eq!(
            registry_for("registry.internal:5000/a/b"),
            "registry.internal:5000"
        );
    }

    /// `localhost` has no dot, so it needs its own clause or it reads as a Hub
    /// namespace and the pull goes to the wrong place entirely.
    #[test]
    fn bare_localhost_is_a_registry() {
        assert_eq!(registry_for("localhost/foo"), "localhost");
    }

    #[test]
    fn hub_is_looked_up_under_its_legacy_v1_key() {
        let keys = lookup_keys("docker.io");
        assert!(keys.contains(&DOCKER_HUB_KEY.to_string()), "{keys:?}");
    }

    #[test]
    fn other_registries_do_not_get_the_hub_key() {
        let keys = lookup_keys("ghcr.io");
        assert!(!keys.contains(&DOCKER_HUB_KEY.to_string()), "{keys:?}");
        assert!(keys.contains(&"https://ghcr.io".to_string()), "{keys:?}");
    }

    #[test]
    fn decodes_a_basic_auth_blob() {
        // "alice:s3cr3t"
        let (user, password) = decode_auth("YWxpY2U6czNjcjN0").expect("decodes");
        assert_eq!(user, "alice");
        assert_eq!(password, "s3cr3t");
    }

    /// Registry passwords are frequently JWTs, which contain colons.
    #[test]
    fn a_password_may_contain_colons() {
        let (user, password) = split_auth("alice:a:b:c").expect("splits");
        assert_eq!(user, "alice");
        assert_eq!(password, "a:b:c");
    }

    #[test]
    fn garbage_auth_is_ignored_rather_than_panicking() {
        assert!(decode_auth("not-base64!!").is_none());
        // Valid base64, but no colon: "hello"
        assert!(decode_auth("aGVsbG8=").is_none());
    }

    #[test]
    fn a_per_registry_helper_beats_the_global_store() {
        let config = AuthConfig {
            creds_store: Some("desktop".into()),
            cred_helpers: HashMap::from([("gcr.io".to_string(), "gcloud".to_string())]),
            ..Default::default()
        };
        assert_eq!(helper_for(&config, "gcr.io"), Some("gcloud"));
        assert_eq!(helper_for(&config, "ghcr.io"), Some("desktop"));
    }

    #[test]
    fn no_helper_configured_means_no_helper() {
        let config = AuthConfig::default();
        assert_eq!(helper_for(&config, "ghcr.io"), None);
    }

    #[test]
    fn keys_are_normalised_to_hostnames_for_display() {
        assert_eq!(normalise_key(DOCKER_HUB_KEY), "docker.io");
        assert_eq!(normalise_key("https://ghcr.io"), "ghcr.io");
        assert_eq!(
            normalise_key("https://registry.example.com/"),
            "registry.example.com"
        );
        assert_eq!(normalise_key("localhost:5000"), "localhost:5000");
    }

    /// A helper name reaches `Command::new`, so anything that is not a plain
    /// identifier must be refused before it can name a path.
    #[tokio::test]
    async fn refuses_a_helper_name_that_is_not_an_identifier() {
        for bad in [
            "../../evil",
            "foo/bar",
            "foo bar",
            "foo;bar",
            "foo$(id)",
            "",
        ] {
            let err = ask_helper(bad, "ghcr.io").await.expect_err(bad);
            assert!(
                matches!(err, AppError::Invalid(_)),
                "{bad} gave {err:?}, expected Invalid"
            );
        }
    }

    #[test]
    fn a_malformed_config_parses_as_nothing_rather_than_failing() {
        let config: Result<AuthConfig, _> = serde_json::from_str("{ not json");
        assert!(config.is_err());
        // And an empty object is valid, with no logins in it.
        let empty: AuthConfig = serde_json::from_str("{}").expect("empty object parses");
        assert!(empty.auths.is_empty());
        assert!(empty.creds_store.is_none());
    }

    fn config(json: &str) -> AuthConfig {
        serde_json::from_str(json).expect("test config parses")
    }

    #[test]
    fn resolves_an_inline_login_to_its_username() {
        // ghcr.io -> alice:s3cr3t
        let c = config(r#"{"auths":{"ghcr.io":{"auth":"YWxpY2U6czNjcjN0"}}}"#);
        let id = identity_in(&c, "config.json", "ghcr.io");
        assert_eq!(id.username.as_deref(), Some("alice"));
        assert_eq!(id.source.as_deref(), Some("config.json"));
    }

    /// The Hub entry is filed under a v1 URL, but a user types `nginx`.
    #[test]
    fn resolves_docker_hub_through_its_legacy_key() {
        let c = config(r#"{"auths":{"https://index.docker.io/v1/":{"auth":"YWxpY2U6czNjcjN0"}}}"#);
        let id = identity_in(&c, "config.json", &registry_for("nginx:alpine"));
        assert_eq!(id.registry, "docker.io");
        assert_eq!(id.username.as_deref(), Some("alice"));
    }

    #[test]
    fn a_registry_with_no_entry_is_anonymous() {
        let c = config(r#"{"auths":{"ghcr.io":{"auth":"YWxpY2U6czNjcjN0"}}}"#);
        let id = identity_in(&c, "config.json", "quay.io");
        assert_eq!(id.source, None, "expected anonymous, got {id:?}");
        assert_eq!(id.username, None);
    }

    /// `docker logout` leaves `{"ghcr.io": {}}` behind. Reporting that as a
    /// login would promise an authenticated pull that then 401s.
    #[test]
    fn an_empty_leftover_entry_is_not_a_login() {
        let c = config(r#"{"auths":{"ghcr.io":{}}}"#);
        let id = identity_in(&c, "config.json", "ghcr.io");
        assert_eq!(id.source, None, "expected anonymous, got {id:?}");
    }

    /// A helper owns the secret, so the identity is known to exist but the
    /// username is not readable without prompting for a keychain unlock.
    #[test]
    fn a_helper_backed_registry_reports_the_helper_and_no_username() {
        let c = config(r#"{"credsStore":"desktop","auths":{"ghcr.io":{}}}"#);
        let id = identity_in(&c, "config.json", "ghcr.io");
        assert_eq!(id.source.as_deref(), Some("docker-credential-desktop"));
        assert_eq!(id.username, None);
    }

    #[test]
    fn listing_covers_both_inline_entries_and_helper_only_registries() {
        let c = config(
            r#"{"auths":{"https://index.docker.io/v1/":{"auth":"YWxpY2U6czNjcjN0"}},
                "credHelpers":{"gcr.io":"gcloud"}}"#,
        );
        let logins = logins_in(&c, "config.json");
        assert_eq!(
            logins,
            vec![
                RegistryLogin {
                    registry: "docker.io".into(),
                    username: Some("alice".into()),
                    source: Some("config.json".into()),
                },
                RegistryLogin {
                    registry: "gcr.io".into(),
                    username: None,
                    source: Some("docker-credential-gcloud".into()),
                },
            ]
        );
    }

    #[test]
    fn an_empty_config_lists_nothing() {
        assert!(logins_in(&AuthConfig::default(), "config.json").is_empty());
    }

    /// The exact shape `docker login` writes for a token-based Hub login,
    /// copied from a real `~/.docker/config.json`. The two token stashes must
    /// not appear as registries — they are storage, not somewhere you can pull
    /// from or log out of.
    #[test]
    fn hub_token_stashes_are_not_listed_as_registries() {
        let c = config(
            r#"{"auths":{
                 "https://index.docker.io/v1/":{"auth":"YWxpY2U6czNjcjN0"},
                 "https://index.docker.io/v1/access-token":{"auth":"YWxpY2U6czNjcjN0"},
                 "https://index.docker.io/v1/refresh-token":{"auth":"YWxpY2U6czNjcjN0"}}}"#,
        );
        let logins = logins_in(&c, "config.json");
        assert_eq!(
            logins,
            vec![RegistryLogin {
                registry: "docker.io".into(),
                username: Some("alice".into()),
                source: Some("config.json".into()),
            }],
            "token stashes leaked into the list"
        );
    }

    #[test]
    fn registry_keys_are_hosts_with_an_optional_port() {
        assert!(is_registry_key(DOCKER_HUB_KEY));
        assert!(is_registry_key("ghcr.io"));
        assert!(is_registry_key("https://ghcr.io"));
        assert!(is_registry_key("localhost:5000"));
        assert!(!is_registry_key("https://index.docker.io/v1/access-token"));
        assert!(!is_registry_key("ghcr.io/owner/repo"));
    }

    /// The real Docker config carries keys this struct does not model; they must
    /// not make the whole file unreadable.
    #[test]
    fn unknown_config_keys_are_ignored() {
        let config: AuthConfig = serde_json::from_str(
            r#"{"auths":{"ghcr.io":{"auth":"YWxpY2U6czNjcjN0"}},
                "currentContext":"desktop-linux","plugins":{"x":{}}}"#,
        )
        .expect("parses");
        assert!(config.auths.contains_key("ghcr.io"));
    }
}
