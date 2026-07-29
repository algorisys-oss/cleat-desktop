//! Applying and deleting YAML manifests.
//!
//! # Why this is dynamic rather than typed
//!
//! Everything in [`super::resources`] knows its type at compile time — a `Pod`
//! is a `Pod`. A manifest is whatever the user pasted, including CRDs this
//! binary has never heard of, so the type has to be discovered at runtime from
//! the document's own `apiVersion`/`kind` and resolved against the cluster's
//! API server. `kube::discovery` is what does that resolution, and it is the
//! reason a CRD applied by an operator works here without Cleat knowing it
//! exists.
//!
//! # Why every apply is dry-run first
//!
//! Apply is the one operation in Cleat that can change a cluster in unbounded
//! ways, from a document the user may have pasted without reading. A server-side
//! dry-run runs the whole admission chain — validation, webhooks, defaulting —
//! and reports exactly what would happen, without persisting anything. Showing
//! that before asking for confirmation turns "apply this YAML and find out" into
//! a decision. It is the same instinct as naming every container in the bulk
//! remove dialog, applied to something with a much larger blast radius.

use crate::error::{AppError, AppResult};
use kube::api::{Api, DeleteParams, Patch, PatchParams, PostParams};
use kube::core::{DynamicObject, GroupVersionKind, TypeMeta};
use kube::discovery::{Discovery, Scope};
use kube::{Client, ResourceExt};
use serde::{Deserialize, Serialize};

/// What one document in a manifest would do, or did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestOutcome {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    /// `created`, `configured`, `unchanged`, `deleted`, or `failed`.
    pub action: String,
    /// Present when `action` is `failed`. The API server's own message, which
    /// for a rejected manifest is far more precise than anything phrased here.
    pub error: Option<String>,
}

/// Field manager name, which is what server-side apply records as the owner of
/// the fields Cleat sets. Distinct from `kubectl`'s so that a later `kubectl
/// apply` reports an honest conflict rather than silently taking ownership.
const FIELD_MANAGER: &str = "cleat";

/// Split a multi-document YAML string into objects.
///
/// Empty documents are normal — a file ending in `---`, or comments between
/// resources — and must be skipped rather than reported as errors.
fn parse_documents(yaml: &str) -> AppResult<Vec<DynamicObject>> {
    let mut objects = Vec::new();
    for (index, doc) in serde_yaml::Deserializer::from_str(yaml).enumerate() {
        let value = serde_yaml::Value::deserialize(doc)
            .map_err(|e| AppError::Invalid(format!("document {}: {e}", index + 1)))?;
        if value.is_null() {
            continue;
        }
        let object: DynamicObject = serde_yaml::from_value(value)
            .map_err(|e| AppError::Invalid(format!("document {}: {e}", index + 1)))?;
        if object.types.is_none() {
            return Err(AppError::Invalid(format!(
                "document {} has no apiVersion/kind",
                index + 1
            )));
        }
        objects.push(object);
    }

    if objects.is_empty() {
        return Err(AppError::Invalid(
            "no Kubernetes resources found in that manifest".into(),
        ));
    }
    Ok(objects)
}

fn gvk_of(types: &TypeMeta) -> AppResult<GroupVersionKind> {
    GroupVersionKind::try_from(types.clone()).map_err(|e| {
        AppError::Invalid(format!(
            "unrecognised apiVersion/kind {}/{}: {e}",
            types.api_version, types.kind
        ))
    })
}

/// Resolve a document to the API endpoint that serves it.
///
/// `default_namespace` is used only for namespaced resources that do not name
/// one, matching `kubectl -n`. A cluster-scoped resource ignores it, which is
/// why the scope has to be discovered rather than assumed.
async fn api_for(
    client: Client,
    discovery: &Discovery,
    object: &DynamicObject,
    default_namespace: Option<&str>,
) -> AppResult<Api<DynamicObject>> {
    let types = object
        .types
        .as_ref()
        .ok_or_else(|| AppError::Invalid("document has no apiVersion/kind".into()))?;
    let gvk = gvk_of(types)?;

    let (resource, capabilities) = discovery.resolve_gvk(&gvk).ok_or_else(|| {
        AppError::NotFound(format!(
            "the cluster does not serve {}/{} — is the CRD installed?",
            types.api_version, types.kind
        ))
    })?;

    Ok(match capabilities.scope {
        Scope::Cluster => Api::all_with(client, &resource),
        Scope::Namespaced => {
            let ns = object
                .metadata
                .namespace
                .as_deref()
                .or(default_namespace)
                .unwrap_or("default");
            Api::namespaced_with(client, ns, &resource)
        }
    })
}

fn describe(object: &DynamicObject) -> (String, String, Option<String>) {
    (
        object
            .types
            .as_ref()
            .map(|t| t.kind.clone())
            .unwrap_or_default(),
        object.name_any(),
        object.metadata.namespace.clone(),
    )
}

/// Apply every document, optionally without persisting anything.
///
/// `dry_run` sends `dryRun=All`, which runs admission and validation server-side
/// and discards the result. The outcomes it reports are what a real apply would
/// do — that is the whole point, and it is why the UI can show them before
/// asking.
pub async fn apply(
    client: Client,
    yaml: &str,
    default_namespace: Option<&str>,
    dry_run: bool,
) -> AppResult<Vec<ManifestOutcome>> {
    let objects = parse_documents(yaml)?;
    let discovery = Discovery::new(client.clone())
        .run()
        .await
        .map_err(|e| AppError::Other(format!("discovering cluster APIs: {e}")))?;

    let mut params = PatchParams::apply(FIELD_MANAGER);
    if dry_run {
        params = params.dry_run();
    }

    let mut outcomes = Vec::new();
    for object in objects {
        let (kind, name, namespace) = describe(&object);
        let api = match api_for(client.clone(), &discovery, &object, default_namespace).await {
            Ok(api) => api,
            Err(e) => {
                outcomes.push(ManifestOutcome {
                    kind,
                    name,
                    namespace,
                    action: "failed".into(),
                    error: Some(e.message()),
                });
                continue;
            }
        };

        // Whether this creates or updates is the server's call, so ask what
        // exists first rather than inferring it from the patch result.
        let existed = api.get_opt(&name).await.ok().flatten().is_some();

        match api.patch(&name, &params, &Patch::Apply(&object)).await {
            Ok(_) => outcomes.push(ManifestOutcome {
                kind,
                name,
                namespace,
                action: if existed { "configured" } else { "created" }.into(),
                error: None,
            }),
            Err(e) => outcomes.push(ManifestOutcome {
                kind,
                name,
                namespace,
                action: "failed".into(),
                error: Some(e.to_string()),
            }),
        }
    }

    Ok(outcomes)
}

/// Delete everything a manifest describes.
///
/// Order is as written, which matters: a manifest that creates a namespace and
/// then resources in it deletes cleanly in the same order only because deleting
/// the namespace takes its contents with it. Reversing would be no safer and
/// less predictable.
pub async fn delete(
    client: Client,
    yaml: &str,
    default_namespace: Option<&str>,
) -> AppResult<Vec<ManifestOutcome>> {
    let objects = parse_documents(yaml)?;
    let discovery = Discovery::new(client.clone())
        .run()
        .await
        .map_err(|e| AppError::Other(format!("discovering cluster APIs: {e}")))?;

    let mut outcomes = Vec::new();
    for object in objects {
        let (kind, name, namespace) = describe(&object);
        let api = match api_for(client.clone(), &discovery, &object, default_namespace).await {
            Ok(api) => api,
            Err(e) => {
                outcomes.push(ManifestOutcome {
                    kind,
                    name,
                    namespace,
                    action: "failed".into(),
                    error: Some(e.message()),
                });
                continue;
            }
        };

        match api.delete(&name, &DeleteParams::default()).await {
            Ok(_) => outcomes.push(ManifestOutcome {
                kind,
                name,
                namespace,
                action: "deleted".into(),
                error: None,
            }),
            // Already gone is the desired state, not a failure — deleting a
            // manifest twice should be quiet rather than alarming.
            Err(kube::Error::Api(e)) if e.code == 404 => outcomes.push(ManifestOutcome {
                kind,
                name,
                namespace,
                action: "unchanged".into(),
                error: None,
            }),
            Err(e) => outcomes.push(ManifestOutcome {
                kind,
                name,
                namespace,
                action: "failed".into(),
                error: Some(e.to_string()),
            }),
        }
    }

    Ok(outcomes)
}

/// Create a namespace if it is missing, so a generated manifest can name one.
pub async fn ensure_namespace(client: Client, name: &str) -> AppResult<()> {
    use k8s_openapi::api::core::v1::Namespace;
    let api: Api<Namespace> = Api::all(client);
    if api.get_opt(name).await.ok().flatten().is_some() {
        return Ok(());
    }
    let ns = Namespace {
        metadata: kube::core::ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    api.create(&PostParams::default(), &ns)
        .await
        .map_err(|e| AppError::Other(format!("creating namespace {name}: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_multi_document_manifest() {
        let yaml = r#"
apiVersion: v1
kind: Namespace
metadata:
  name: demo
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: demo
spec:
  replicas: 1
"#;
        let objects = parse_documents(yaml).expect("parses");
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].types.as_ref().unwrap().kind, "Namespace");
        assert_eq!(objects[1].name_any(), "web");
    }

    /// A file that ends in `---`, or has a comment between resources, yields
    /// empty documents. Reporting those as errors would reject valid manifests.
    #[test]
    fn empty_documents_are_skipped() {
        let yaml = "---\napiVersion: v1\nkind: Namespace\nmetadata:\n  name: demo\n---\n";
        let objects = parse_documents(yaml).expect("parses");
        assert_eq!(objects.len(), 1);
    }

    #[test]
    fn a_document_without_a_kind_is_rejected() {
        let err = parse_documents("metadata:\n  name: nope\n").expect_err("should reject");
        assert!(
            matches!(err, AppError::Invalid(_)),
            "expected Invalid, got {err:?}"
        );
    }

    #[test]
    fn an_empty_manifest_is_rejected_rather_than_silently_doing_nothing() {
        for empty in ["", "\n", "---\n", "# just a comment\n"] {
            let err = parse_documents(empty).expect_err(&format!("{empty:?} should reject"));
            assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
        }
    }

    #[test]
    fn malformed_yaml_names_the_document_that_failed() {
        let yaml = "apiVersion: v1\nkind: Namespace\n---\n  bad: [indent\n";
        let err = parse_documents(yaml).expect_err("should reject");
        assert!(
            err.message().contains("document 2"),
            "error should name the failing document, got {err:?}"
        );
    }
}
