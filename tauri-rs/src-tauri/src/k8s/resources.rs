//! Listing cluster resources, reduced to what the tables render.
//!
//! Every function here takes a client and a namespace selector, so the command
//! layer never has to know how a client is built. `None` for a namespace means
//! all namespaces, matching `kubectl -A`.

use super::model::{ConfigEntry, Deployment, Event, Namespace, Node, Pod, Service};
use crate::error::{AppError, AppResult};
use k8s_openapi::api::apps::v1::Deployment as K8sDeployment;
use k8s_openapi::api::core::v1::{
    Namespace as K8sNamespace, Node as K8sNode, Pod as K8sPod, Service as K8sService,
};
use kube::api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams};
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

/// One-shot log fetch, `tail` lines.
pub async fn pod_logs(
    client: Client,
    namespace: &str,
    name: &str,
    container: Option<&str>,
    tail: i64,
) -> AppResult<String> {
    let api: Api<K8sPod> = Api::namespaced(client, namespace);
    let params = LogParams {
        container: container.map(str::to_string),
        tail_lines: Some(tail),
        ..Default::default()
    };
    api.logs(name, &params)
        .await
        .map_err(|e| AppError::Other(format!("logs for {name}: {e}")))
}

/// A followed pod log, one line per item.
///
/// Same shape as [`LogStream`](crate::runtime::LogStream) so the command layer
/// pumps it identically — note it is `futures`' `AsyncBufRead` that `log_stream`
/// returns, not tokio's, and `futures`' `lines()` already yields a `Stream`.
pub type PodLogStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = AppResult<String>> + Send>>;

/// Follow a pod's logs.
///
/// Unlike the container path there is no stdout/stderr split: the API server
/// merges them and does not say which was which, so the UI must not offer a
/// filter it cannot honour.
pub async fn follow_pod_logs(
    client: Client,
    namespace: &str,
    name: &str,
    container: Option<&str>,
    tail: i64,
) -> AppResult<PodLogStream> {
    use futures_util::{AsyncBufReadExt, StreamExt};

    let api: Api<K8sPod> = Api::namespaced(client, namespace);
    let params = LogParams {
        container: container.map(str::to_string),
        tail_lines: Some(tail),
        follow: true,
        ..Default::default()
    };

    let reader = api
        .log_stream(name, &params)
        .await
        .map_err(|e| AppError::Other(format!("following logs for {name}: {e}")))?;

    Ok(Box::pin(reader.lines().map(|line| {
        line.map_err(|e| AppError::Other(format!("reading log stream: {e}")))
    })))
}

/// Recent events, newest first.
///
/// Sorted by last-seen rather than creation: a warning that fired once an hour
/// ago and again ten seconds ago is current, and ordering by creation buries it.
pub async fn list_events(client: Client, namespace: Option<&str>) -> AppResult<Vec<Event>> {
    use k8s_openapi::api::core::v1::Event as K8sEvent;

    let api: Api<K8sEvent> = api_for(client, namespace);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("events", e))?;

    let mut events: Vec<Event> = list
        .items
        .into_iter()
        .map(|e| {
            // `last_timestamp` is the classic field; newer clusters may only
            // populate `event_time`, and falling back keeps events from
            // vanishing on those.
            let seen = e
                .last_timestamp
                .as_ref()
                .map(|t| t.0.as_second())
                .or_else(|| e.event_time.as_ref().map(|t| t.0.as_second()))
                .unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(seen);

            Event {
                namespace: e.metadata.namespace.clone().unwrap_or_default(),
                type_: e.type_.unwrap_or_else(|| "Normal".into()),
                reason: e.reason.unwrap_or_default(),
                message: e.message.unwrap_or_default(),
                object: format!(
                    "{}/{}",
                    e.involved_object.kind.unwrap_or_default(),
                    e.involved_object.name.unwrap_or_default()
                ),
                count: e.count.unwrap_or(1),
                age: if seen == 0 { 0 } else { (now - seen).max(0) },
            }
        })
        .collect();

    events.sort_by_key(|e| e.age);
    Ok(events)
}

pub async fn list_configmaps(
    client: Client,
    namespace: Option<&str>,
) -> AppResult<Vec<ConfigEntry>> {
    use k8s_openapi::api::core::v1::ConfigMap;

    let api: Api<ConfigMap> = api_for(client, namespace);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("config maps", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|cm| ConfigEntry {
            name: cm.name_any(),
            namespace: cm.namespace().unwrap_or_default(),
            kind: "ConfigMap".into(),
            type_: None,
            keys: cm.data.map(|d| d.into_keys().collect()).unwrap_or_default(),
            age: age_of(&cm.metadata),
        })
        .collect())
}

/// Secrets, as names and key names only.
///
/// The values are deliberately never read, let alone returned. Cleat's whole
/// stance on credentials is that it does not hold them — reading a Secret's
/// contents into the app would put cluster credentials in a process that had no
/// reason to have them, and one screenshot away from a bug report. Knowing
/// *which* keys exist is what the UI actually needs.
pub async fn list_secrets(client: Client, namespace: Option<&str>) -> AppResult<Vec<ConfigEntry>> {
    use k8s_openapi::api::core::v1::Secret;

    let api: Api<Secret> = api_for(client, namespace);
    let list = api
        .list(&ListParams::default())
        .await
        .map_err(|e| list_error("secrets", e))?;

    Ok(list
        .items
        .into_iter()
        .map(|s| ConfigEntry {
            name: s.name_any(),
            namespace: s.namespace().unwrap_or_default(),
            kind: "Secret".into(),
            type_: s.type_.clone(),
            keys: s.data.map(|d| d.into_keys().collect()).unwrap_or_default(),
            age: age_of(&s.metadata),
        })
        .collect())
}

/// Set a deployment's replica count.
pub async fn scale_deployment(
    client: Client,
    namespace: &str,
    name: &str,
    replicas: i32,
) -> AppResult<()> {
    if replicas < 0 {
        return Err(AppError::Invalid("replicas cannot be negative".into()));
    }
    let api: Api<K8sDeployment> = Api::namespaced(client, namespace);
    let patch = serde_json::json!({ "spec": { "replicas": replicas } });
    api.patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(|e| AppError::Other(format!("scaling {name}: {e}")))?;
    Ok(())
}

/// Roll a deployment's pods, the way `kubectl rollout restart` does.
///
/// There is no restart verb in the API. What the CLI actually does is stamp an
/// annotation on the pod template, which changes the template hash and makes
/// the controller roll out new pods — a real rolling restart rather than
/// deleting pods and hoping. Reimplemented here rather than shelling out to
/// kubectl, which Cleat does not depend on.
pub async fn restart_deployment(client: Client, namespace: &str, name: &str) -> AppResult<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let api: Api<K8sDeployment> = Api::namespaced(client, namespace);
    let patch = serde_json::json!({
        "spec": { "template": { "metadata": { "annotations": {
            "cleat.dev/restartedAt": now.to_string()
        }}}}
    });
    api.patch(name, &PatchParams::default(), &Patch::Merge(&patch))
        .await
        .map_err(|e| AppError::Other(format!("restarting {name}: {e}")))?;
    Ok(())
}

/// StatefulSets, DaemonSets, Jobs and CronJobs in one list.
///
/// Fetched concurrently: four sequential round trips to the API server is a
/// visible pause on a remote cluster, and they do not depend on each other.
/// A kind the cluster refuses (RBAC, or an old server) contributes nothing
/// rather than failing the whole list — a user who can see Jobs but not
/// CronJobs should still see Jobs.
pub async fn list_workloads(
    client: Client,
    namespace: Option<&str>,
) -> AppResult<Vec<crate::k8s::model::Workload>> {
    use crate::k8s::model::Workload;
    use k8s_openapi::api::apps::v1::{DaemonSet, StatefulSet};
    use k8s_openapi::api::batch::v1::{CronJob, Job};

    // Bound to locals first: `join!` borrows its arguments, and an `Api` built
    // inline is a temporary that dies before the future is polled.
    let stateful_api = api_for::<StatefulSet>(client.clone(), namespace);
    let daemon_api = api_for::<DaemonSet>(client.clone(), namespace);
    let job_api = api_for::<Job>(client.clone(), namespace);
    let cron_api = api_for::<CronJob>(client, namespace);
    let params = ListParams::default();

    let (stateful, daemon, jobs, cronjobs) = tokio::join!(
        stateful_api.list(&params),
        daemon_api.list(&params),
        job_api.list(&params),
        cron_api.list(&params),
    );

    let mut out = Vec::new();

    if let Ok(list) = stateful {
        for s in list.items {
            let desired = s.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
            let ready = s
                .status
                .as_ref()
                .and_then(|s| s.ready_replicas)
                .unwrap_or(0);
            out.push(Workload {
                kind: "StatefulSet".into(),
                name: s.name_any(),
                namespace: s.namespace().unwrap_or_default(),
                ready: format!("{ready}/{desired}"),
                detail: s
                    .spec
                    .as_ref()
                    .map(|sp| sp.service_name.clone().unwrap_or_default())
                    .unwrap_or_default(),
                age: age_of(&s.metadata),
            });
        }
    }

    if let Ok(list) = daemon {
        for d in list.items {
            let status = d.status.as_ref();
            // A DaemonSet has no replica count: its "desired" is however many
            // nodes match, which the status reports and the spec does not.
            let desired = status.map(|s| s.desired_number_scheduled).unwrap_or(0);
            let ready = status.map(|s| s.number_ready).unwrap_or(0);
            out.push(Workload {
                kind: "DaemonSet".into(),
                name: d.name_any(),
                namespace: d.namespace().unwrap_or_default(),
                ready: format!("{ready}/{desired}"),
                detail: format!(
                    "{} up to date",
                    status.and_then(|s| s.updated_number_scheduled).unwrap_or(0)
                ),
                age: age_of(&d.metadata),
            });
        }
    }

    if let Ok(list) = jobs {
        for j in list.items {
            let status = j.status.as_ref();
            let succeeded = status.and_then(|s| s.succeeded).unwrap_or(0);
            let wanted = j.spec.as_ref().and_then(|s| s.completions).unwrap_or(1);
            let failed = status.and_then(|s| s.failed).unwrap_or(0);
            out.push(Workload {
                kind: "Job".into(),
                name: j.name_any(),
                namespace: j.namespace().unwrap_or_default(),
                ready: format!("{succeeded}/{wanted}"),
                detail: if failed > 0 {
                    format!("{failed} failed")
                } else {
                    String::new()
                },
                age: age_of(&j.metadata),
            });
        }
    }

    if let Ok(list) = cronjobs {
        for c in list.items {
            let spec = c.spec.as_ref();
            let active = c
                .status
                .as_ref()
                .and_then(|s| s.active.as_ref())
                .map(|a| a.len())
                .unwrap_or(0);
            out.push(Workload {
                kind: "CronJob".into(),
                name: c.name_any(),
                namespace: c.namespace().unwrap_or_default(),
                ready: format!("{active} active"),
                detail: spec
                    .map(|s| {
                        let suspended = s.suspend.unwrap_or(false);
                        if suspended {
                            format!("{} (suspended)", s.schedule)
                        } else {
                            s.schedule.clone()
                        }
                    })
                    .unwrap_or_default(),
                age: age_of(&c.metadata),
            });
        }
    }

    out.sort_by(|a, b| {
        a.namespace
            .cmp(&b.namespace)
            .then(a.kind.cmp(&b.kind))
            .then(a.name.cmp(&b.name))
    });
    Ok(out)
}
