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

/// A dry run must report what would happen and change nothing.
///
/// This is the assertion the whole apply flow rests on: the UI disables Apply
/// until a dry run comes back clean, so a dry run that silently created things
/// would make that gate worse than useless.
#[tokio::test]
async fn a_dry_run_reports_outcomes_without_creating_anything() {
    let Some(client) = client().await else { return };
    const NS: &str = "cleat-test-dryrun";
    let yaml = format!("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {NS}\n");

    let outcomes = cleat_lib::k8s::manifests::apply(client.clone(), &yaml, None, true)
        .await
        .expect("dry run");
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].action, "created", "{outcomes:?}");

    let namespaces = resources::list_namespaces(client)
        .await
        .expect("list namespaces");
    assert!(
        !namespaces.iter().any(|n| n.name == NS),
        "the dry run actually created {NS}"
    );
}

/// Apply, confirm it landed, then delete — the full round trip against a real
/// API server, including the discovery step that resolves kind to endpoint.
#[tokio::test]
async fn applies_and_deletes_a_manifest() {
    let Some(client) = client().await else { return };
    const NS: &str = "cleat-test-apply";
    let yaml = format!(
        "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {NS}\n  labels:\n    cleat-test: \"true\"\n"
    );

    let created = cleat_lib::k8s::manifests::apply(client.clone(), &yaml, None, false)
        .await
        .expect("apply");
    assert_eq!(created[0].action, "created", "{created:?}");

    let namespaces = resources::list_namespaces(client.clone())
        .await
        .expect("list namespaces");
    assert!(
        namespaces.iter().any(|n| n.name == NS),
        "{NS} absent after a successful apply"
    );

    // Re-applying the same document is `configured`, not a second `created` —
    // server-side apply is idempotent and the outcome has to say so.
    let again = cleat_lib::k8s::manifests::apply(client.clone(), &yaml, None, false)
        .await
        .expect("re-apply");
    assert_eq!(again[0].action, "configured", "{again:?}");

    let deleted = cleat_lib::k8s::manifests::delete(client.clone(), &yaml, None)
        .await
        .expect("delete");
    assert_eq!(deleted[0].action, "deleted", "{deleted:?}");

    // Deleting what is already gone is the desired state, not a failure.
    let twice = cleat_lib::k8s::manifests::delete(client, &yaml, None)
        .await
        .expect("delete twice");
    assert!(
        twice[0].action == "unchanged" || twice[0].action == "deleted",
        "second delete should be quiet, got {twice:?}"
    );
}

/// A kind the cluster does not serve must fail with a message naming it,
/// rather than a discovery panic or a generic 404.
#[tokio::test]
async fn an_unknown_kind_fails_with_a_useful_message() {
    let Some(client) = client().await else { return };
    let yaml = "apiVersion: nonsense.example.com/v1\nkind: Widget\nmetadata:\n  name: w\n";
    let outcomes = cleat_lib::k8s::manifests::apply(client, yaml, None, true)
        .await
        .expect("apply returns per-document outcomes rather than failing whole");
    assert_eq!(outcomes[0].action, "failed", "{outcomes:?}");
    let error = outcomes[0].error.as_deref().unwrap_or_default();
    assert!(
        error.contains("Widget") || error.contains("nonsense.example.com"),
        "error should name the unresolvable kind, got {error:?}"
    );
}

#[tokio::test]
async fn lists_events() {
    let Some(client) = client().await else { return };
    let events = resources::list_events(client, None)
        .await
        .expect("list events");
    for e in &events {
        assert!(!e.object.is_empty(), "event with no involved object: {e:?}");
        assert!(
            e.type_ == "Normal" || e.type_ == "Warning",
            "unexpected event type {:?}",
            e.type_
        );
    }
    // Newest first: ages must be non-decreasing down the list.
    for pair in events.windows(2) {
        assert!(
            pair[0].age <= pair[1].age,
            "events out of order: {} then {}",
            pair[0].age,
            pair[1].age
        );
    }
}

/// Secrets must come back as key *names* with no values anywhere in the DTO.
#[tokio::test]
async fn secrets_are_listed_without_their_values() {
    let Some(client) = client().await else { return };
    let secrets = resources::list_secrets(client, None)
        .await
        .expect("list secrets");
    let Some(secret) = secrets.iter().find(|s| !s.keys.is_empty()) else {
        eprintln!("skipping: no secrets with data on this cluster");
        return;
    };
    // The DTO has no field for a value at all, so this asserts the shape that
    // makes leaking one impossible rather than that a particular value is absent.
    let json = serde_json::to_string(secret).expect("serialise");
    assert!(json.contains("\"keys\""), "{json}");
    assert!(
        !json.contains("\"data\""),
        "a secret DTO carried data: {json}"
    );
    assert!(
        !json.contains("\"value\""),
        "a secret DTO carried a value: {json}"
    );
}

/// Scale down and back up on a deployment this test creates.
#[tokio::test]
async fn scales_and_restarts_a_deployment() {
    let Some(client) = client().await else { return };
    const NS: &str = "default";
    const NAME: &str = "cleat-test-scale";
    let yaml = format!(
        r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: {NAME}
  namespace: {NS}
spec:
  replicas: 1
  selector:
    matchLabels:
      app: {NAME}
  template:
    metadata:
      labels:
        app: {NAME}
    spec:
      containers:
        - name: c
          image: busybox
          command: ["sh", "-c", "sleep 3600"]
"#
    );

    cleat_lib::k8s::manifests::apply(client.clone(), &yaml, None, false)
        .await
        .expect("create deployment");

    let outcome = async {
        resources::scale_deployment(client.clone(), NS, NAME, 2).await?;
        let after = resources::list_deployments(client.clone(), Some(NS)).await?;
        let d = after
            .iter()
            .find(|d| d.name == NAME)
            .expect("deployment present after scale");
        assert!(
            d.ready.ends_with("/2"),
            "expected a desired count of 2, got {:?}",
            d.ready
        );

        // Restart is a template annotation, not a delete — it must succeed and
        // leave the deployment in place.
        resources::restart_deployment(client.clone(), NS, NAME).await?;
        let still = resources::list_deployments(client.clone(), Some(NS)).await?;
        assert!(
            still.iter().any(|d| d.name == NAME),
            "restart removed the deployment"
        );
        Ok::<(), cleat_lib::error::AppError>(())
    }
    .await;

    let _ = cleat_lib::k8s::manifests::delete(client, &yaml, None).await;
    outcome.expect("scale/restart");
}
