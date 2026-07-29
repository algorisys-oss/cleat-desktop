//! DTOs for the Kubernetes views.
//!
//! Kept out of [`crate::model`] because this is a different domain, not more of
//! the same one. A container runtime and a cluster share almost no vocabulary —
//! there is no image list on a cluster and no namespace on a container — so
//! folding these in would produce one module that is really two.
//!
//! # Why hand-written summaries rather than the upstream types
//!
//! `k8s-openapi` already models every field of a `Pod`, and serialising one
//! straight to the frontend is tempting. It is also 40 kB of JSON per pod,
//! almost all of it managed fields and status conditions nobody renders, and it
//! binds the UI to the API version the crate was generated against. These carry
//! what the tables show. The full object is available on demand through the
//! inspect path, which returns untyped JSON exactly like the container one.

use serde::Serialize;

/// One entry from the kubeconfig.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KubeContext {
    pub name: String,
    pub cluster: String,
    pub user: String,
    /// The context's own default namespace, when it declares one.
    pub namespace: Option<String>,
    pub current: bool,
}

/// Whether a cluster is actually reachable, and what it is.
///
/// Mirrors [`RuntimeInfo`](crate::model::RuntimeInfo): a context that exists in
/// the kubeconfig but whose cluster is down must be visibly unavailable with the
/// reason attached, rather than appearing selectable and failing on first use.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterInfo {
    pub context: String,
    pub available: bool,
    /// Server version, when the cluster answered.
    pub version: Option<String>,
    /// Why it is unavailable, when it is.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Namespace {
    pub name: String,
    pub phase: String,
    /// Seconds since creation, or 0 when the cluster did not report it.
    pub age: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pod {
    pub name: String,
    pub namespace: String,
    /// `Running`, `Pending`, `Succeeded`, `Failed`, `Unknown` — or a container
    /// state like `CrashLoopBackOff` when one is more informative than the
    /// phase, which is what `kubectl get pods` shows in its STATUS column.
    pub status: String,
    pub ready: String,
    pub restarts: i32,
    pub age: i64,
    pub node: Option<String>,
    pub ip: Option<String>,
    /// Container names, needed to pick a stream for logs.
    pub containers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub name: String,
    pub namespace: String,
    pub ready: String,
    pub up_to_date: i32,
    pub available: i32,
    pub age: i64,
    pub images: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub name: String,
    pub namespace: String,
    pub type_: String,
    pub cluster_ip: Option<String>,
    pub external_ip: Option<String>,
    pub ports: Vec<String>,
    pub age: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub name: String,
    pub status: String,
    pub roles: Vec<String>,
    pub version: String,
    pub age: i64,
    pub internal_ip: Option<String>,
}
