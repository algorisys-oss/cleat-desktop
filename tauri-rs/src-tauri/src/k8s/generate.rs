//! Turning running containers into Kubernetes manifests.
//!
//! This is the one place the container and cluster halves of Cleat meet, and
//! only in this direction. It takes the daemon's own inspect output — the same
//! JSON the inspect panel renders — and produces YAML for review.
//!
//! # Why review rather than apply
//!
//! The translation is lossy and cannot not be. A container has a host, a
//! cluster does not; a bind mount to `/home/you/src` means nothing on a node
//! that is not your laptop; `--privileged` maps onto a security context most
//! clusters reject. Generating YAML and applying it in one motion hides all of
//! that until it fails at admission. So this returns a document, the UI shows
//! it, and applying is a separate decision — with a server-side dry-run in
//! between. [`Warning`] carries what was dropped or guessed so the reader is
//! told rather than left to notice.
//!
//! # Deployment, not Pod
//!
//! `podman generate kube` emits a bare Pod, which is faithful to what is running
//! and almost never what you want on a cluster: nothing restarts it, nothing
//! rolls it, and it cannot scale. A Deployment is the smallest thing that
//! behaves like a workload, so that is what this emits, plus a Service when the
//! container publishes ports.

use crate::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::Value;

/// Something the reader needs to know about the translation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    /// Machine-readable so the UI can style by severity without parsing prose.
    pub kind: String,
    pub message: String,
}

impl Warning {
    fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Generated {
    pub yaml: String,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub namespace: Option<String>,
    pub replicas: i32,
    /// Emit a Service alongside the Deployment when ports are exposed.
    pub include_service: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            namespace: None,
            replicas: 1,
            include_service: true,
        }
    }
}

/// Environment keys whose values should not be pasted into a manifest.
///
/// Deliberately the same list the activity log redacts by, for the same reason:
/// a missed secret ends up in a file someone commits, and an over-eager warning
/// costs a sentence. Unlike the activity log this does **not** redact — a
/// Deployment without its environment does not run — it warns, and leaves the
/// decision with the reader.
const SECRET_HINTS: [&str; 8] = [
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "credential",
    "auth",
];

fn looks_secret(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SECRET_HINTS.iter().any(|h| lowered.contains(h))
        || lowered == "key"
        || lowered.ends_with("_key")
}

/// Coerce a container name into an RFC 1123 label.
///
/// Kubernetes names are far stricter than container names: lowercase
/// alphanumerics and dashes, starting and ending alphanumeric, 63 characters.
/// A compose container called `Oracle_Frontend_1` is legal to Docker and
/// rejected by the API server, so this is not cosmetic — without it every
/// generated manifest for a compose project fails at admission.
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.trim_start_matches('/').chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '_' | '-' | '.' | ' ' | '/') {
            out.push('-');
        }
        // Anything else is dropped rather than transliterated: a name is an
        // identifier here, not a label, and inventing characters would produce
        // collisions that are hard to notice.
    }

    let trimmed = out.trim_matches('-').to_string();
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut last_dash = false;
    for ch in trimmed.chars() {
        if ch == '-' {
            if !last_dash {
                collapsed.push(ch);
            }
            last_dash = true;
        } else {
            collapsed.push(ch);
            last_dash = false;
        }
    }

    let mut result: String = collapsed.chars().take(63).collect();
    result = result.trim_matches('-').to_string();
    if result.is_empty() || !result.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        result = format!("app-{result}").trim_matches('-').to_string();
    }
    result
}

/// `KEY=VALUE` strings from the daemon, as env entries.
fn env_from(config: &Value, warnings: &mut Vec<Warning>) -> Vec<(String, String)> {
    let Some(entries) = config.get("Env").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut env = Vec::new();
    let mut secretish = Vec::new();
    for entry in entries.iter().filter_map(Value::as_str) {
        let Some((key, value)) = entry.split_once('=') else {
            continue;
        };
        // PATH and friends come from the image, not from the user, and copying
        // them into a manifest overrides whatever the image sets on the cluster.
        if key == "PATH" || key == "HOSTNAME" || key == "HOME" || key == "TERM" {
            continue;
        }
        if looks_secret(key) && !value.is_empty() {
            secretish.push(key.to_string());
        }
        env.push((key.to_string(), value.to_string()));
    }

    if !secretish.is_empty() {
        warnings.push(Warning::new(
            "secret-in-env",
            format!(
                "{} environment value{} look like secrets ({}). They are written into this \
                 manifest in plain text — move them to a Secret before committing it.",
                secretish.len(),
                if secretish.len() == 1 { "" } else { "s" },
                secretish.join(", ")
            ),
        ));
    }
    env
}

/// Container ports, from the exposed set the image declares plus anything
/// published on the host.
fn ports_from(config: &Value, host_config: &Value, warnings: &mut Vec<Warning>) -> Vec<(i32, String)> {
    let mut ports: Vec<(i32, String)> = Vec::new();

    let mut add = |spec: &str| {
        // Docker writes these as "80/tcp".
        let (number, proto) = match spec.split_once('/') {
            Some((n, p)) => (n, p.to_ascii_uppercase()),
            None => (spec, "TCP".to_string()),
        };
        if let Ok(port) = number.parse::<i32>() {
            if !ports.iter().any(|(p, _)| *p == port) {
                ports.push((port, proto));
            }
        }
    };

    if let Some(exposed) = config.get("ExposedPorts").and_then(Value::as_object) {
        for spec in exposed.keys() {
            add(spec);
        }
    }
    if let Some(bindings) = host_config.get("PortBindings").and_then(Value::as_object) {
        for spec in bindings.keys() {
            add(spec);
        }
        if !bindings.is_empty() {
            warnings.push(Warning::new(
                "host-ports-dropped",
                "Host port mappings do not translate: a Service is emitted instead, reachable \
                 inside the cluster. Change its type to NodePort or LoadBalancer to reach it \
                 from outside.",
            ));
        }
    }

    ports.sort_by_key(|(p, _)| *p);
    ports
}

/// Report mounts, which cannot be translated faithfully.
fn warn_about_mounts(inspect: &Value, warnings: &mut Vec<Warning>) {
    let Some(mounts) = inspect.get("Mounts").and_then(Value::as_array) else {
        return;
    };
    let mut binds = Vec::new();
    let mut volumes = Vec::new();
    for mount in mounts {
        match mount.get("Type").and_then(Value::as_str) {
            Some("bind") => {
                if let Some(source) = mount.get("Source").and_then(Value::as_str) {
                    binds.push(source.to_string());
                }
            }
            Some("volume") => {
                if let Some(name) = mount.get("Name").and_then(Value::as_str) {
                    volumes.push(name.to_string());
                }
            }
            _ => {}
        }
    }

    if !binds.is_empty() {
        warnings.push(Warning::new(
            "bind-mounts-dropped",
            format!(
                "{} bind mount{} dropped ({}). A path on this machine does not exist on a \
                 cluster node — use a ConfigMap, a Secret, or a PersistentVolumeClaim.",
                binds.len(),
                if binds.len() == 1 { " was" } else { "s were" },
                binds.join(", ")
            ),
        ));
    }
    if !volumes.is_empty() {
        warnings.push(Warning::new(
            "volumes-dropped",
            format!(
                "{} named volume{} dropped ({}). Add a PersistentVolumeClaim if this workload \
                 needs to keep data.",
                volumes.len(),
                if volumes.len() == 1 { " was" } else { "s were" },
                volumes.join(", ")
            ),
        ));
    }
}

fn string_list(config: &Value, key: &str) -> Vec<String> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Build a Deployment (and optionally a Service) from a container's inspect JSON.
pub fn from_inspect(inspect: &Value, options: &Options) -> AppResult<Generated> {
    let config = inspect.get("Config").cloned().unwrap_or(Value::Null);
    let host_config = inspect.get("HostConfig").cloned().unwrap_or(Value::Null);

    let raw_name = inspect
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = sanitize_name(raw_name);
    if name.is_empty() {
        return Err(AppError::Invalid(
            "the container has no name to derive a manifest name from".into(),
        ));
    }

    let image = config
        .get("Image")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Invalid("the container reports no image".into()))?
        .to_string();

    let mut warnings = Vec::new();
    if sanitize_name(raw_name) != raw_name.trim_start_matches('/') {
        warnings.push(Warning::new(
            "name-rewritten",
            format!(
                "Renamed {:?} to {name:?}: Kubernetes names are lowercase alphanumerics and \
                 dashes only.",
                raw_name.trim_start_matches('/')
            ),
        ));
    }
    if image.contains("sha256:") || !image.contains('/') && !image.contains(':') {
        warnings.push(Warning::new(
            "image-not-portable",
            format!(
                "The cluster must be able to pull {image:?}. A locally built image is not \
                 visible to a remote cluster unless you push it or load it into the node."
            ),
        ));
    }

    let env = env_from(&config, &mut warnings);
    let ports = ports_from(&config, &host_config, &mut warnings);
    warn_about_mounts(inspect, &mut warnings);

    if host_config
        .get("Privileged")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        warnings.push(Warning::new(
            "privileged",
            "This container runs privileged. The manifest requests the same, which most \
             clusters refuse — remove it unless the workload genuinely needs it.",
        ));
    }

    let entrypoint = string_list(&config, "Entrypoint");
    let cmd = string_list(&config, "Cmd");

    let yaml = render(
        &name,
        &image,
        options,
        &env,
        &ports,
        &entrypoint,
        &cmd,
        host_config
            .get("Privileged")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );

    Ok(Generated { yaml, warnings })
}

/// Emit the YAML.
///
/// Written by hand rather than serialised from `k8s-openapi` types. Those
/// serialise every field they model, so a Deployment comes out carrying dozens
/// of nulls and empty objects the reader has to look past — and this document
/// exists to be read before it is applied.
#[allow(clippy::too_many_arguments)]
fn render(
    name: &str,
    image: &str,
    options: &Options,
    env: &[(String, String)],
    ports: &[(i32, String)],
    entrypoint: &[String],
    cmd: &[String],
    privileged: bool,
) -> String {
    let mut out = String::new();
    let ns_line = options
        .namespace
        .as_deref()
        .map(|ns| format!("  namespace: {ns}\n"))
        .unwrap_or_default();

    out.push_str("apiVersion: apps/v1\n");
    out.push_str("kind: Deployment\n");
    out.push_str("metadata:\n");
    out.push_str(&format!("  name: {name}\n"));
    out.push_str(&ns_line);
    out.push_str("  labels:\n");
    out.push_str(&format!("    app: {name}\n"));
    out.push_str("spec:\n");
    out.push_str(&format!("  replicas: {}\n", options.replicas.max(0)));
    out.push_str("  selector:\n    matchLabels:\n");
    out.push_str(&format!("      app: {name}\n"));
    out.push_str("  template:\n");
    out.push_str("    metadata:\n      labels:\n");
    out.push_str(&format!("        app: {name}\n"));
    out.push_str("    spec:\n      containers:\n");
    out.push_str(&format!("        - name: {name}\n"));
    out.push_str(&format!("          image: {}\n", yaml_scalar(image)));

    // Docker's Entrypoint is Kubernetes' `command`, and Docker's Cmd is `args`.
    // Getting this backwards produces a container that starts and immediately
    // does the wrong thing, which is worse than one that fails to start.
    if !entrypoint.is_empty() {
        out.push_str("          command:\n");
        for part in entrypoint {
            out.push_str(&format!("            - {}\n", yaml_scalar(part)));
        }
    }
    if !cmd.is_empty() {
        out.push_str("          args:\n");
        for part in cmd {
            out.push_str(&format!("            - {}\n", yaml_scalar(part)));
        }
    }

    if !ports.is_empty() {
        out.push_str("          ports:\n");
        for (port, proto) in ports {
            out.push_str(&format!("            - containerPort: {port}\n"));
            out.push_str(&format!("              protocol: {proto}\n"));
        }
    }

    if !env.is_empty() {
        out.push_str("          env:\n");
        for (key, value) in env {
            out.push_str(&format!("            - name: {key}\n"));
            out.push_str(&format!("              value: {}\n", yaml_scalar(value)));
        }
    }

    if privileged {
        out.push_str("          securityContext:\n            privileged: true\n");
    }

    if options.include_service && !ports.is_empty() {
        out.push_str("---\n");
        out.push_str("apiVersion: v1\n");
        out.push_str("kind: Service\n");
        out.push_str("metadata:\n");
        out.push_str(&format!("  name: {name}\n"));
        out.push_str(&ns_line);
        out.push_str("spec:\n");
        out.push_str("  selector:\n");
        out.push_str(&format!("    app: {name}\n"));
        out.push_str("  ports:\n");
        for (port, proto) in ports {
            out.push_str(&format!("    - name: port-{port}\n"));
            out.push_str(&format!("      port: {port}\n"));
            out.push_str(&format!("      targetPort: {port}\n"));
            out.push_str(&format!("      protocol: {proto}\n"));
        }
    }

    out
}

/// Quote a scalar when YAML would otherwise misread it.
///
/// An env value of `yes`, `1.0` or `on` parses as a boolean or a number, and a
/// value containing `: ` starts a mapping. Quoting only when needed keeps the
/// document readable, which is the point of emitting it for review.
fn yaml_scalar(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.trim() != value
        || value.contains(": ")
        || value.contains('#')
        || value.starts_with(['&', '*', '!', '|', '>', '%', '@', '`', '{', '[', '\'', '"'])
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
        )
        || value.parse::<f64>().is_ok();

    if needs_quotes {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize as _;
    use serde_json::json;

    fn inspect(name: &str, config: Value, host_config: Value) -> Value {
        json!({ "Name": name, "Config": config, "HostConfig": host_config })
    }

    #[test]
    fn sanitizes_names_kubernetes_would_reject() {
        assert_eq!(sanitize_name("/oracle_frontend_1"), "oracle-frontend-1");
        assert_eq!(sanitize_name("Web.Server"), "web-server");
        assert_eq!(sanitize_name("--weird--"), "weird");
        assert_eq!(sanitize_name("tickethub/control_plane"), "tickethub-control-plane");
    }

    /// Consecutive separators must collapse; `a__b` becoming `a--b` is legal but
    /// ugly, and `-a` is outright rejected by the API server.
    #[test]
    fn collapses_separators_and_fixes_the_first_character() {
        assert_eq!(sanitize_name("a__b"), "a-b");
        assert_eq!(sanitize_name("_leading"), "leading");
        assert_eq!(sanitize_name("123abc"), "123abc");
    }

    #[test]
    fn names_are_capped_at_the_label_limit() {
        let long = "x".repeat(200);
        assert_eq!(sanitize_name(&long).len(), 63);
    }

    #[test]
    fn a_name_of_only_punctuation_still_produces_something_valid() {
        let name = sanitize_name("!!!");
        assert!(!name.is_empty());
        assert!(name.starts_with(|c: char| c.is_ascii_alphanumeric()), "{name}");
    }

    #[test]
    fn generates_a_deployment_and_a_service() {
        let v = inspect(
            "/web",
            json!({ "Image": "nginx:alpine", "ExposedPorts": { "80/tcp": {} } }),
            json!({}),
        );
        let out = from_inspect(&v, &Options::default()).expect("generates");
        assert!(out.yaml.contains("kind: Deployment"), "{}", out.yaml);
        assert!(out.yaml.contains("kind: Service"), "{}", out.yaml);
        assert!(out.yaml.contains("image: nginx:alpine"), "{}", out.yaml);
        assert!(out.yaml.contains("containerPort: 80"), "{}", out.yaml);
    }

    #[test]
    fn no_ports_means_no_service() {
        let v = inspect("/worker", json!({ "Image": "busybox" }), json!({}));
        let out = from_inspect(&v, &Options::default()).expect("generates");
        assert!(!out.yaml.contains("kind: Service"), "{}", out.yaml);
    }

    /// Docker's Entrypoint is Kubernetes' `command` and Docker's Cmd is `args`.
    /// Swapping them yields a container that starts and does the wrong thing.
    #[test]
    fn entrypoint_becomes_command_and_cmd_becomes_args() {
        let v = inspect(
            "/app",
            json!({
                "Image": "busybox",
                "Entrypoint": ["/bin/sh", "-c"],
                "Cmd": ["echo hello"]
            }),
            json!({}),
        );
        let out = from_inspect(&v, &Options::default()).expect("generates");
        let command_at = out.yaml.find("command:").expect("command emitted");
        let args_at = out.yaml.find("args:").expect("args emitted");
        assert!(command_at < args_at, "{}", out.yaml);
        assert!(out.yaml.contains("- /bin/sh"), "{}", out.yaml);
        assert!(out.yaml.contains("- echo hello"), "{}", out.yaml);
    }

    #[test]
    fn image_environment_noise_is_not_copied() {
        let v = inspect(
            "/app",
            json!({
                "Image": "busybox",
                "Env": ["PATH=/usr/bin", "HOSTNAME=abc", "APP_MODE=production"]
            }),
            json!({}),
        );
        let out = from_inspect(&v, &Options::default()).expect("generates");
        assert!(out.yaml.contains("APP_MODE"), "{}", out.yaml);
        assert!(!out.yaml.contains("PATH"), "{}", out.yaml);
        assert!(!out.yaml.contains("HOSTNAME"), "{}", out.yaml);
    }

    #[test]
    fn secrets_in_the_environment_are_warned_about_but_kept() {
        let v = inspect(
            "/app",
            json!({
                "Image": "busybox",
                "Env": ["DB_PASSWORD=hunter2", "API_TOKEN=abc", "APP_MODE=prod"]
            }),
            json!({}),
        );
        let out = from_inspect(&v, &Options::default()).expect("generates");
        // Kept, because a Deployment without its environment does not run.
        assert!(out.yaml.contains("hunter2"), "{}", out.yaml);
        let warning = out
            .warnings
            .iter()
            .find(|w| w.kind == "secret-in-env")
            .unwrap_or_else(|| panic!("no secret warning in {:?}", out.warnings));
        assert!(warning.message.contains("DB_PASSWORD"), "{warning:?}");
        assert!(warning.message.contains("API_TOKEN"), "{warning:?}");
        assert!(!warning.message.contains("APP_MODE"), "{warning:?}");
    }

    #[test]
    fn bind_mounts_and_volumes_are_reported_as_dropped() {
        let mut v = inspect("/app", json!({ "Image": "busybox" }), json!({}));
        v["Mounts"] = json!([
            { "Type": "bind", "Source": "/home/rajesh/src" },
            { "Type": "volume", "Name": "app-data" }
        ]);
        let out = from_inspect(&v, &Options::default()).expect("generates");
        assert!(out.warnings.iter().any(|w| w.kind == "bind-mounts-dropped"));
        assert!(out.warnings.iter().any(|w| w.kind == "volumes-dropped"));
    }

    #[test]
    fn a_renamed_container_says_so() {
        let v = inspect("/Oracle_Frontend_1", json!({ "Image": "busybox" }), json!({}));
        let out = from_inspect(&v, &Options::default()).expect("generates");
        let warning = out
            .warnings
            .iter()
            .find(|w| w.kind == "name-rewritten")
            .unwrap_or_else(|| panic!("no rename warning in {:?}", out.warnings));
        assert!(warning.message.contains("oracle-frontend-1"), "{warning:?}");
    }

    #[test]
    fn privileged_containers_are_flagged() {
        let v = inspect(
            "/app",
            json!({ "Image": "busybox" }),
            json!({ "Privileged": true }),
        );
        let out = from_inspect(&v, &Options::default()).expect("generates");
        assert!(out.yaml.contains("privileged: true"), "{}", out.yaml);
        assert!(out.warnings.iter().any(|w| w.kind == "privileged"));
    }

    /// An env value of `yes` or `1.0` parses as a bool or a number, which
    /// changes what the container receives.
    #[test]
    fn ambiguous_scalars_are_quoted() {
        assert_eq!(yaml_scalar("yes"), "\"yes\"");
        assert_eq!(yaml_scalar("1.0"), "\"1.0\"");
        assert_eq!(yaml_scalar("true"), "\"true\"");
        assert_eq!(yaml_scalar("null"), "\"null\"");
        assert_eq!(yaml_scalar(""), "\"\"");
        assert_eq!(yaml_scalar("plain"), "plain");
        assert_eq!(yaml_scalar("nginx:alpine"), "nginx:alpine");
    }

    #[test]
    fn a_container_without_an_image_is_an_error() {
        let v = inspect("/app", json!({}), json!({}));
        let err = from_inspect(&v, &Options::default()).expect_err("should reject");
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }

    /// The whole document has to survive a YAML parse, or "review then apply"
    /// is a lie — the apply path parses it again.
    #[test]
    fn the_generated_document_parses_as_yaml() {
        let v = inspect(
            "/app",
            json!({
                "Image": "nginx:alpine",
                "ExposedPorts": { "80/tcp": {}, "443/tcp": {} },
                "Env": ["MODE=yes", "GREETING=hello: world"],
                "Entrypoint": ["/docker-entrypoint.sh"],
                "Cmd": ["nginx", "-g", "daemon off;"]
            }),
            json!({ "PortBindings": { "80/tcp": [{ "HostPort": "8080" }] } }),
        );
        let out = from_inspect(&v, &Options::default()).expect("generates");

        let docs: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(&out.yaml)
            .map(|d| serde_yaml::Value::deserialize(d).expect("each document parses"))
            .collect();
        assert_eq!(docs.len(), 2, "expected Deployment + Service:\n{}", out.yaml);

        // And the tricky scalars survived as strings rather than being coerced.
        let env = docs[0]
            .get("spec")
            .and_then(|s| s.get("template"))
            .and_then(|t| t.get("spec"))
            .and_then(|s| s.get("containers"))
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("env"))
            .and_then(|e| e.as_sequence())
            .expect("env present");
        let mode = env
            .iter()
            .find(|e| e.get("name").and_then(|n| n.as_str()) == Some("MODE"))
            .expect("MODE present");
        assert_eq!(mode.get("value").and_then(|v| v.as_str()), Some("yes"));
    }
}
