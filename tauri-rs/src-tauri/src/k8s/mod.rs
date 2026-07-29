//! Kubernetes support.
//!
//! # Why this is not a `ContainerRuntime`
//!
//! The obvious move is to add a third backend next to Docker and Podman. It
//! does not fit: [`ContainerRuntime`](crate::runtime::ContainerRuntime) is 40
//! methods about containers, images, networks and volumes, and a cluster has
//! none of those as first-class objects. A pod is not a container, a deployment
//! has no equivalent at all, and `list_images` is meaningless against an API
//! server. Forcing it in would mean forty `Err(Unsupported)` stubs and a trait
//! that no longer describes anything.
//!
//! So this is a parallel subsystem with its own client, its own DTOs and its own
//! commands. The two meet in exactly one place — generating manifests from a
//! container or a compose project — and that direction is one-way.
//!
//! # Connecting
//!
//! `kube` reads the same kubeconfig `kubectl` does, including its auth plugins.
//! That matters for the same reason [`crate::runtime::credentials`] reads
//! Docker's config rather than storing its own: a cluster you can already reach
//! from the terminal should work here without being set up twice.

pub mod model;
pub mod resources;

use crate::error::{AppError, AppResult};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use model::{ClusterInfo, KubeContext};

/// Read the kubeconfig, or report why it could not be read.
///
/// A missing file is not an error worth surfacing loudly — plenty of machines
/// have no cluster — but a malformed one is, because it means the file is there
/// and wrong, which the user can fix.
fn load_kubeconfig() -> AppResult<Kubeconfig> {
    Kubeconfig::read()
        .map_err(|e| AppError::RuntimeUnavailable(format!("could not read kubeconfig: {e}")))
}

/// Every context in the kubeconfig.
///
/// Reading the file only; no cluster is contacted. Contacting one per context
/// would make opening the view as slow as the least reachable cluster in it.
pub fn contexts() -> AppResult<Vec<KubeContext>> {
    let config = load_kubeconfig()?;
    let current = config.current_context.clone();

    Ok(config
        .contexts
        .iter()
        .map(|named| {
            let ctx = named.context.as_ref();
            KubeContext {
                name: named.name.clone(),
                cluster: ctx.map(|c| c.cluster.clone()).unwrap_or_default(),
                user: ctx.and_then(|c| c.user.clone()).unwrap_or_default(),
                namespace: ctx.and_then(|c| c.namespace.clone()),
                current: Some(&named.name) == current.as_ref(),
            }
        })
        .collect())
}

/// Build a client for `context`, or for the kubeconfig's current context.
///
/// Constructed per call rather than cached. A client holds a TLS connection
/// whose credentials may be minted by an exec plugin with a short expiry, so a
/// long-lived cached client is one that works until the token quietly expires
/// and then fails in a way that looks like the cluster went down.
pub async fn client(context: Option<&str>) -> AppResult<Client> {
    let options = KubeConfigOptions {
        context: context.map(str::to_string),
        ..Default::default()
    };
    let config = Config::from_kubeconfig(&options)
        .await
        .map_err(|e| AppError::RuntimeUnavailable(format!("kubeconfig: {e}")))?;

    Client::try_from(config)
        .map_err(|e| AppError::RuntimeUnavailable(format!("kubernetes client: {e}")))
}

/// Contact `context`'s cluster and report whether it answered.
///
/// The equivalent of [`crate::runtime::docker::probe`]: the switcher needs to
/// distinguish "configured" from "reachable", and say why when they differ.
pub async fn probe(context: &str) -> ClusterInfo {
    let unavailable = |detail: String| ClusterInfo {
        context: context.to_string(),
        available: false,
        version: None,
        detail: Some(detail),
    };

    let client = match client(Some(context)).await {
        Ok(c) => c,
        Err(e) => return unavailable(e.message()),
    };

    match client.apiserver_version().await {
        Ok(v) => ClusterInfo {
            context: context.to_string(),
            available: true,
            version: Some(v.git_version),
            detail: None,
        },
        Err(e) => unavailable(format!("cluster unreachable: {e}")),
    }
}

/// Probe every context, concurrently.
///
/// Sequentially this is as slow as the sum of the unreachable ones, each of
/// which costs a connection timeout.
pub async fn probe_all() -> AppResult<Vec<ClusterInfo>> {
    let contexts = contexts()?;
    Ok(futures_util::future::join_all(contexts.iter().map(|c| probe(&c.name))).await)
}
