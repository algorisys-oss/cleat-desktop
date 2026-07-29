//! Listing cluster resources, reduced to what the tables render.
//!
//! Every function here takes a client and a namespace selector, so the command
//! layer never has to know how a client is built. `None` for a namespace means
//! all namespaces, matching `kubectl -A`.

use super::model::{Deployment, Namespace, Node, Pod, Service};
use crate::error::{AppError, AppResult};
use k8s_openapi::api::apps::v1::Deployment as K8sDeployment;
use k8s_openapi::api::core::v1::{
    Namespace as K8sNamespace, Node as K8sNode, Pod as K8sPod, Service as K8sService,
};
use kube::api::{Api, DeleteParams, ListParams};
use kube::{Client, ResourceExt};

/// Seconds since `creation_timestamp`.
///
/// The UI formats ages itself, so this hands over a duration rather than a
/// formatted string — the same contract the container list uses.
fn age_of(meta: &kube::core::ObjectMeta) -> i64 {
    meta.creation_timestamp
        .as_ref()
        .map(|t| {
            // k8s-openapi 0.28 carries a `jiff::Timestamp`, not a chrono one.
            let created = t.0.as_second();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(created);
            (now - created).max(0)
        })
        .unwrap_or(0)
}

fn api_for<K>(client: Client, namespace: Option<&str>) -> Api<K>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope>,
    <K as kube::Resource>::DynamicType: Default,
{
    match namespace {
        Some(ns) => Api::namespaced(client, ns),
        None => Api::all(client),
    }
}

fn list_error(kind: &str, e: kube::Error) -> AppError {
    AppError::Other(format!("listing {kind}: {e}"))
}

pub async fn list_namespaces(client: Client) -> AppResult<Vec<Namespace>> {
    let api: Api<K8sNamespace> = Api::all(client);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("namespaces", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|ns| Namespace {
            name: ns.name_any(),
            phase: ns
                .status
                .as_ref()
                .and_then(|s| s.phase.clone())
                .unwrap_or_else(|| "Unknown".into()),
            age: age_of(&ns.metadata),
        })
        .collect())
}

/// What `kubectl get pods` puts in its STATUS column.
///
/// Not simply `status.phase`. A pod stuck pulling an image or crash-looping is
/// `Pending`/`Running` by phase, which tells you nothing — the useful string is
/// on the container state, and showing the phase instead is why "why is this pod
/// broken" so often means dropping to the CLI. Deletion wins over both: a pod
/// with a deletion timestamp is `Terminating` regardless of what it was doing.
fn pod_status(pod: &K8sPod) -> String {
    if pod.metadata.deletion_timestamp.is_some() {
        return "Terminating".into();
    }

    let statuses = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref());

    if let Some(statuses) = statuses {
        for cs in statuses {
            let Some(state) = cs.state.as_ref() else {
                continue;
            };
            if let Some(waiting) = state.waiting.as_ref() {
                if let Some(reason) = waiting.reason.as_ref() {
                    // `ContainerCreating` and `PodInitializing` are the normal
                    // path to Running, not problems, so they read better as the
                    // phase they belong to.
                    if reason != "ContainerCreating" && reason != "PodInitializing" {
                        return reason.clone();
                    }
                }
            }
            if let Some(terminated) = state.terminated.as_ref() {
                if let Some(reason) = terminated.reason.as_ref() {
                    if reason != "Completed" {
                        return reason.clone();
                    }
                }
            }
        }
    }

    pod.status
        .as_ref()
        .and_then(|s| s.phase.clone())
        .unwrap_or_else(|| "Unknown".into())
}

/// `ready/total` containers, the way `kubectl` reports it.
fn pod_ready(pod: &K8sPod) -> (String, i32) {
    let statuses = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref());

    let Some(statuses) = statuses else {
        let total = pod.spec.as_ref().map(|s| s.containers.len()).unwrap_or(0);
        return (format!("0/{total}"), 0);
    };

    let ready = statuses.iter().filter(|c| c.ready).count();
    let restarts = statuses.iter().map(|c| c.restart_count).max().unwrap_or(0);
    (format!("{ready}/{}", statuses.len()), restarts)
}

pub async fn list_pods(client: Client, namespace: Option<&str>) -> AppResult<Vec<Pod>> {
    let api: Api<K8sPod> = api_for(client, namespace);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("pods", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|pod| {
            let (ready, restarts) = pod_ready(&pod);
            Pod {
                name: pod.name_any(),
                namespace: pod.namespace().unwrap_or_default(),
                status: pod_status(&pod),
                ready,
                restarts,
                age: age_of(&pod.metadata),
                node: pod.spec.as_ref().and_then(|s| s.node_name.clone()),
                ip: pod.status.as_ref().and_then(|s| s.pod_ip.clone()),
                containers: pod
                    .spec
                    .as_ref()
                    .map(|s| s.containers.iter().map(|c| c.name.clone()).collect())
                    .unwrap_or_default(),
            }
        })
        .collect())
}

pub async fn list_deployments(
    client: Client,
    namespace: Option<&str>,
) -> AppResult<Vec<Deployment>> {
    let api: Api<K8sDeployment> = api_for(client, namespace);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("deployments", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|d| {
            let status = d.status.as_ref();
            let desired = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
            let ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
            Deployment {
                name: d.name_any(),
                namespace: d.namespace().unwrap_or_default(),
                ready: format!("{ready}/{desired}"),
                up_to_date: status.and_then(|s| s.updated_replicas).unwrap_or(0),
                available: status.and_then(|s| s.available_replicas).unwrap_or(0),
                age: age_of(&d.metadata),
                images: d
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.spec.as_ref())
                    .map(|s| {
                        s.containers
                            .iter()
                            .filter_map(|c| c.image.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect())
}

pub async fn list_services(client: Client, namespace: Option<&str>) -> AppResult<Vec<Service>> {
    let api: Api<K8sService> = api_for(client, namespace);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("services", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|s| {
            let spec = s.spec.as_ref();
            Service {
                name: s.name_any(),
                namespace: s.namespace().unwrap_or_default(),
                type_: spec
                    .and_then(|s| s.type_.clone())
                    .unwrap_or_else(|| "ClusterIP".into()),
                cluster_ip: spec.and_then(|s| s.cluster_ip.clone()),
                // A LoadBalancer that has not been assigned an address yet
                // reports nothing here, which the UI renders as "pending"
                // rather than inventing a value.
                external_ip: s
                    .status
                    .as_ref()
                    .and_then(|st| st.load_balancer.as_ref())
                    .and_then(|lb| lb.ingress.as_ref())
                    .and_then(|ing| ing.first())
                    .and_then(|i| i.ip.clone().or_else(|| i.hostname.clone())),
                ports: spec
                    .and_then(|s| s.ports.as_ref())
                    .map(|ports| {
                        ports
                            .iter()
                            .map(|p| match p.node_port {
                                Some(np) => {
                                    format!("{}:{np}/{}", p.port, protocol(p.protocol.as_ref()))
                                }
                                None => format!("{}/{}", p.port, protocol(p.protocol.as_ref())),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                age: age_of(&s.metadata),
            }
        })
        .collect())
}

fn protocol(p: Option<&String>) -> String {
    p.cloned().unwrap_or_else(|| "TCP".into())
}

pub async fn list_nodes(client: Client) -> AppResult<Vec<Node>> {
    let api: Api<K8sNode> = Api::all(client);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("nodes", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|n| {
            let status = n.status.as_ref();
            Node {
                name: n.name_any(),
                // A node's readiness is a condition, not a field: find the
                // `Ready` one and report its status, which is "True", "False"
                // or "Unknown" — the last meaning the kubelet stopped
                // reporting, which is different from being unready.
                status: status
                    .and_then(|s| s.conditions.as_ref())
                    .and_then(|c| c.iter().find(|c| c.type_ == "Ready"))
                    .map(|c| match c.status.as_str() {
                        "True" => "Ready".to_string(),
                        "False" => "NotReady".to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "Unknown".into()),
                // Roles live in labels, one label per role, which is why this
                // is a list rather than a field.
                roles: n
                    .metadata
                    .labels
                    .as_ref()
                    .map(|labels| {
                        labels
                            .keys()
                            .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/"))
                            .filter(|r| !r.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                version: status
                    .and_then(|s| s.node_info.as_ref())
                    .map(|i| i.kubelet_version.clone())
                    .unwrap_or_default(),
                age: age_of(&n.metadata),
                internal_ip: status
                    .and_then(|s| s.addresses.as_ref())
                    .and_then(|a| a.iter().find(|a| a.type_ == "InternalIP"))
                    .map(|a| a.address.clone()),
            }
        })
        .collect())
}

/// Full object as untyped JSON, for the inspect panel.
pub async fn inspect_pod(
    client: Client,
    namespace: &str,
    name: &str,
) -> AppResult<serde_json::Value> {
    let api: Api<K8sPod> = Api::namespaced(client, namespace);
    let pod = api
        .get(name)
        .await
        .map_err(|e| AppError::Other(format!("inspecting pod {name}: {e}")))?;
    serde_json::to_value(pod).map_err(|e| AppError::Other(e.to_string()))
}

/// Delete a pod.
///
/// The controller that owns it will recreate it, which is why this is the
/// ordinary way to restart a workload and why it is the one mutation in the
/// read-only layer.
pub async fn delete_pod(client: Client, namespace: &str, name: &str) -> AppResult<()> {
    let api: Api<K8sPod> = Api::namespaced(client, namespace);
    api.delete(name, &DeleteParams::default())
        .await
        .map_err(|e| AppError::Other(format!("deleting pod {name}: {e}")))?;
    Ok(())
}
