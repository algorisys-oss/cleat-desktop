//! Kubernetes integration tests against a real cluster.
//!
//! Skips cleanly when there is no reachable cluster, the same contract the
//! Docker and Podman suites follow — most machines running these tests have no
//! kubeconfig at all, and that must not be a failure.
//!
//! Read-only apart from `deletes_a_pod`, which creates its own pod first and
//! removes it again. Nothing here touches a resource it did not create.

use cleat_lib::k8s;
use cleat_lib::k8s::resources;
use kube::Client;

/// A client for the current context, or None if there is no cluster to talk to.
async fn client() -> Option<Client> {
    match k8s::client(None).await {
        Ok(c) => match c.apiserver_version().await {
            Ok(_) => Some(c),
            Err(e) => {
                eprintln!("skipping: cluster unreachable ({e})");
                None
            }
        },
        Err(e) => {
            eprintln!("skipping: no usable kubeconfig ({})", e.message());
            None
        }
    }
}

#[tokio::test]
async fn reads_contexts_from_the_kubeconfig() {
    let contexts = match k8s::contexts() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: no kubeconfig ({})", e.message());
            return;
        }
    };
    for c in &contexts {
        assert!(!c.name.is_empty(), "a context must have a name");
        assert!(!c.cluster.is_empty(), "context {} names no cluster", c.name);
    }
    assert!(
        contexts.iter().filter(|c| c.current).count() <= 1,
        "at most one context can be current: {contexts:?}"
    );
}

#[tokio::test]
async fn probes_report_a_version_or_a_reason() {
    let Ok(infos) = k8s::probe_all().await else {
        eprintln!("skipping: no kubeconfig");
        return;
    };
    for info in &infos {
        // The switcher renders one or the other; neither being present would
        // leave a row that says nothing.
        assert!(
            info.version.is_some() || info.detail.is_some(),
            "{} reported neither a version nor a reason",
            info.context
        );
        assert_eq!(
            info.available,
            info.version.is_some(),
            "{} claims available={} with version={:?}",
            info.context,
            info.available,
            info.version
        );
    }
}

#[tokio::test]
async fn lists_namespaces() {
    let Some(client) = client().await else { return };
    let namespaces = resources::list_namespaces(client)
        .await
        .expect("list namespaces");
    assert!(
        namespaces.iter().any(|n| n.name == "kube-system"),
        "every cluster has kube-system: {namespaces:?}"
    );
    for ns in &namespaces {
        assert!(!ns.phase.is_empty(), "namespace {} has no phase", ns.name);
    }
}

#[tokio::test]
async fn lists_pods_across_all_namespaces() {
    let Some(client) = client().await else { return };
    let pods = resources::list_pods(client, None).await.expect("list pods");
    // kube-system always runs something; an empty list means the namespace
    // selector is wrong rather than that the cluster is idle.
    assert!(
        !pods.is_empty(),
        "no pods in any namespace, which no working cluster does"
    );
    for pod in &pods {
        assert!(!pod.name.is_empty());
        assert!(
            !pod.namespace.is_empty(),
            "pod {} has no namespace",
            pod.name
        );
        assert!(!pod.status.is_empty(), "pod {} has no status", pod.name);
        assert!(
            pod.ready.contains('/'),
            "pod {} ready should be ready/total, got {:?}",
            pod.name,
            pod.ready
        );
    }
}

/// Namespacing is the difference between `kubectl get pods` and `-A`, and
/// getting it backwards is invisible on a single-namespace cluster.
#[tokio::test]
async fn a_namespace_selector_narrows_the_result() {
    let Some(client) = client().await else { return };
    let all = resources::list_pods(client.clone(), None)
        .await
        .expect("list all pods");
    let system = resources::list_pods(client, Some("kube-system"))
        .await
        .expect("list kube-system pods");

    assert!(
        system.iter().all(|p| p.namespace == "kube-system"),
        "namespaced list returned pods from elsewhere"
    );
    assert!(
        system.len() <= all.len(),
        "one namespace cannot hold more pods than every namespace"
    );
}

#[tokio::test]
async fn lists_nodes_with_roles_and_versions() {
    let Some(client) = client().await else { return };
    let nodes = resources::list_nodes(client).await.expect("list nodes");
    assert!(!nodes.is_empty(), "a cluster has at least one node");
    for node in &nodes {
        assert!(!node.name.is_empty());
        assert!(
            !node.version.is_empty(),
            "node {} reports no kubelet version",
            node.name
        );
    }
}

#[tokio::test]
async fn lists_services_including_the_default_kubernetes_one() {
    let Some(client) = client().await else { return };
    let services = resources::list_services(client, None)
        .await
        .expect("list services");
    assert!(
        services
            .iter()
            .any(|s| s.name == "kubernetes" && s.namespace == "default"),
        "every cluster exposes default/kubernetes: {services:?}"
    );
}

#[tokio::test]
async fn lists_deployments() {
    let Some(client) = client().await else { return };
    let deployments = resources::list_deployments(client, None)
        .await
        .expect("list deployments");
    for d in &deployments {
        assert!(
            d.ready.contains('/'),
            "deployment {} ready should be ready/desired, got {:?}",
            d.name,
            d.ready
        );
    }
}

#[tokio::test]
async fn inspects_a_pod_as_json() {
    let Some(client) = client().await else { return };
    let pods = resources::list_pods(client.clone(), Some("kube-system"))
        .await
        .expect("list pods");
    let Some(pod) = pods.first() else {
        eprintln!("skipping: kube-system has no pods");
        return;
    };

    let json = resources::inspect_pod(client, &pod.namespace, &pod.name)
        .await
        .expect("inspect pod");
    assert_eq!(
        json.pointer("/metadata/name").and_then(|v| v.as_str()),
        Some(pod.name.as_str()),
        "inspect returned a different pod"
    );
}
