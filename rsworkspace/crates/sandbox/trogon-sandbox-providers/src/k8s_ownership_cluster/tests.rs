use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use http::{Method, Request, Response};
use http_body_util::BodyExt;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};

use super::{
    apply_exact_network_policy, delete_exact_resource, inspect_resource_metadata, put_exact_owned_secret,
    set_exact_resource_owner,
};
use crate::{K8sOwnershipError, K8sResourceKind, K8sResourceReceipt};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Reply = (u16, Value);

const NAMESPACE: &str = "work";

#[derive(Clone, Debug)]
struct Seen {
    method: Method,
    body: Value,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A kube client backed by an in-process API server whose replies come from `script`.
fn client(script: impl Fn(&Method, &str) -> Reply + Send + Sync + 'static) -> (Client, Arc<Mutex<Vec<Seen>>>) {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let script = Arc::new(script);
    let recorder = Arc::clone(&seen);
    let service = tower::service_fn(move |request: Request<Body>| {
        let script = Arc::clone(&script);
        let recorder = Arc::clone(&recorder);
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect().await?.to_bytes();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let (status, reply) = script(&parts.method, parts.uri.path());
            locked(&recorder).push(Seen {
                method: parts.method,
                body,
            });
            let response = Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&reply)?))?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(response)
        }
    });
    (Client::new(service, NAMESPACE), seen)
}

fn failure(code: u16, reason: &str) -> Reply {
    (
        code,
        json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "message": reason, "reason": reason, "code": code}),
    )
}

fn api_prefix(kind: K8sResourceKind) -> &'static str {
    match kind {
        K8sResourceKind::Job => "/apis/batch/v1",
        K8sResourceKind::NetworkPolicy => "/apis/networking.k8s.io/v1",
        K8sResourceKind::Pod | K8sResourceKind::Secret | K8sResourceKind::ConfigMap => "/api/v1",
    }
}

fn plural(kind: K8sResourceKind) -> &'static str {
    match kind {
        K8sResourceKind::Job => "jobs",
        K8sResourceKind::Pod => "pods",
        K8sResourceKind::Secret => "secrets",
        K8sResourceKind::NetworkPolicy => "networkpolicies",
        K8sResourceKind::ConfigMap => "configmaps",
    }
}

fn object_path(kind: K8sResourceKind, name: &str) -> String {
    format!("{}/namespaces/{NAMESPACE}/{}/{name}", api_prefix(kind), plural(kind))
}

fn collection_path(kind: K8sResourceKind) -> String {
    format!("{}/namespaces/{NAMESPACE}/{}", api_prefix(kind), plural(kind))
}

fn kind_name(kind: K8sResourceKind) -> &'static str {
    match kind {
        K8sResourceKind::Job => "Job",
        K8sResourceKind::Pod => "Pod",
        K8sResourceKind::Secret => "Secret",
        K8sResourceKind::NetworkPolicy => "NetworkPolicy",
        K8sResourceKind::ConfigMap => "ConfigMap",
    }
}

fn object(kind: K8sResourceKind, name: &str, uid: Option<&str>, revision: Option<&str>, owners: Value) -> Value {
    let mut metadata = json!({"name": name, "namespace": NAMESPACE, "ownerReferences": owners});
    if let Some(uid) = uid {
        metadata["uid"] = json!(uid);
    }
    if let Some(revision) = revision {
        metadata["resourceVersion"] = json!(revision);
    }
    let mut value = json!({"kind": kind_name(kind), "metadata": metadata});
    value["apiVersion"] = json!(match kind {
        K8sResourceKind::Job => "batch/v1",
        K8sResourceKind::NetworkPolicy => "networking.k8s.io/v1",
        _ => "v1",
    });
    if kind == K8sResourceKind::NetworkPolicy {
        value["spec"] = json!({"podSelector": {}});
    }
    value
}

fn owner_of(receipt: &K8sResourceReceipt) -> Value {
    let reference = receipt.owner_reference();
    json!([{"apiVersion": reference.api_version, "kind": reference.kind, "name": reference.name, "uid": reference.uid}])
}

fn metadata(name: &str, uid: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some(NAMESPACE.to_string()),
        uid: Some(uid.to_string()),
        ..Default::default()
    }
}

fn receipt(kind: K8sResourceKind, name: &str, uid: &str) -> Result<K8sResourceReceipt, K8sOwnershipError> {
    K8sResourceReceipt::from_metadata(kind, NAMESPACE, &metadata(name, uid))
}

fn only(seen: &Arc<Mutex<Vec<Seen>>>, method: &Method) -> Vec<Seen> {
    locked(seen)
        .iter()
        .filter(|request| request.method == *method)
        .cloned()
        .collect()
}

#[tokio::test]
async fn inspecting_returns_the_metadata_of_an_existing_object() -> TestResult {
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, _) = client(move |_, requested| {
        if requested == path {
            (
                200,
                object(K8sResourceKind::Pod, "agent", Some("u1"), Some("5"), json!([])),
            )
        } else {
            failure(404, "NotFound")
        }
    });

    let found = inspect_resource_metadata(&client, K8sResourceKind::Pod, NAMESPACE, "agent").await?;
    assert_eq!(found.and_then(|found| found.uid), Some("u1".to_string()));
    let missing = inspect_resource_metadata(&client, K8sResourceKind::Pod, NAMESPACE, "other").await?;
    assert!(missing.is_none());
    Ok(())
}

#[tokio::test]
async fn inspecting_surfaces_an_api_failure() {
    let (client, _) = client(|_, _| failure(500, "InternalError"));

    let result = inspect_resource_metadata(&client, K8sResourceKind::Job, NAMESPACE, "agent").await;

    assert!(matches!(result, Err(K8sOwnershipError::Kube(_))));
}

#[tokio::test]
async fn deleting_fences_on_the_uid_and_revision_and_confirms_the_object_is_gone() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, seen) = client(move |method, requested| match *method {
        Method::DELETE if requested == path => {
            (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"}))
        }
        _ => failure(404, "NotFound"),
    });

    delete_exact_resource(&client, &target, Some("7")).await?;

    let deletes = only(&seen, &Method::DELETE);
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].body["preconditions"]["uid"], "u1");
    assert_eq!(deletes[0].body["preconditions"]["resourceVersion"], "7");
    assert_eq!(deletes[0].body["propagationPolicy"], "Background");
    Ok(())
}

#[tokio::test]
async fn deleting_an_object_that_is_already_absent_succeeds() -> TestResult {
    let target = receipt(K8sResourceKind::Secret, "token", "u1")?;
    let (client, _) = client(|_, _| failure(404, "NotFound"));

    delete_exact_resource(&client, &target, None).await?;
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_is_forgiven_when_the_object_is_gone_afterwards() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let (client, _) = client(|method, _| match *method {
        Method::DELETE => failure(409, "Conflict"),
        _ => failure(404, "NotFound"),
    });

    delete_exact_resource(&client, &target, None).await?;
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_of_a_live_object_is_reported() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, _) = client(move |method, requested| match *method {
        Method::DELETE => failure(500, "InternalError"),
        _ if requested == path => (
            200,
            object(K8sResourceKind::Pod, "agent", Some("u1"), Some("5"), json!([])),
        ),
        _ => failure(404, "NotFound"),
    });

    let result = delete_exact_resource(&client, &target, None).await;

    assert!(matches!(result, Err(K8sOwnershipError::Kube(_))));
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_whose_inspection_also_fails_reports_the_inspection_failure() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let (client, _) = client(|_, _| failure(500, "InternalError"));

    let result = delete_exact_resource(&client, &target, None).await;

    assert!(matches!(result, Err(K8sOwnershipError::Kube(_))));
    Ok(())
}

#[tokio::test]
async fn a_delete_that_leaves_the_same_object_in_place_is_not_confirmed() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, _) = client(move |method, requested| match *method {
        Method::DELETE => (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"})),
        _ if requested == path => (
            200,
            object(K8sResourceKind::Pod, "agent", Some("u1"), Some("5"), json!([])),
        ),
        _ => failure(404, "NotFound"),
    });

    let result = delete_exact_resource(&client, &target, None).await;

    assert!(matches!(result, Err(K8sOwnershipError::CleanupNotConfirmed)));
    Ok(())
}

#[tokio::test]
async fn a_replacement_object_with_another_uid_counts_as_gone() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, _) = client(move |method, requested| match *method {
        Method::DELETE => (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"})),
        _ if requested == path => (
            200,
            object(K8sResourceKind::Pod, "agent", Some("u2"), Some("1"), json!([])),
        ),
        _ => failure(404, "NotFound"),
    });

    delete_exact_resource(&client, &target, None).await?;
    Ok(())
}

#[tokio::test]
async fn an_inspection_that_names_another_object_is_rejected() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, _) = client(move |method, requested| match *method {
        Method::DELETE => (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"})),
        _ if requested == path => (
            200,
            object(K8sResourceKind::Pod, "imposter", Some("u1"), Some("1"), json!([])),
        ),
        _ => failure(404, "NotFound"),
    });

    let result = delete_exact_resource(&client, &target, None).await;

    assert!(matches!(result, Err(K8sOwnershipError::InspectionNameMismatch)));
    Ok(())
}

#[tokio::test]
async fn an_inspection_without_a_uid_is_not_a_confirmed_object() -> TestResult {
    let target = receipt(K8sResourceKind::Pod, "agent", "u1")?;
    let path = object_path(K8sResourceKind::Pod, "agent");
    let (client, _) = client(move |method, requested| match *method {
        Method::DELETE => (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"})),
        _ if requested == path => (200, object(K8sResourceKind::Pod, "agent", None, Some("1"), json!([]))),
        _ => failure(404, "NotFound"),
    });

    let result = delete_exact_resource(&client, &target, None).await;

    assert!(matches!(result, Err(K8sOwnershipError::UidNotConfirmed)));
    Ok(())
}

#[tokio::test]
async fn adopting_patches_the_owner_reference_fenced_on_the_uid_and_revision() -> TestResult {
    let child = receipt(K8sResourceKind::Secret, "token", "child-uid")?;
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let path = object_path(K8sResourceKind::Secret, "token");
    let (client, seen) = client(move |_, requested| {
        if requested == path {
            (
                200,
                object(
                    K8sResourceKind::Secret,
                    "token",
                    Some("child-uid"),
                    Some("9"),
                    json!([]),
                ),
            )
        } else {
            failure(404, "NotFound")
        }
    });

    set_exact_resource_owner(&client, &child, &owner).await?;

    let patches = only(&seen, &Method::PATCH);
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0].body["metadata"]["uid"], "child-uid");
    assert_eq!(patches[0].body["metadata"]["resourceVersion"], "9");
    assert_eq!(patches[0].body["metadata"]["ownerReferences"][0]["uid"], "owner-uid");
    assert_eq!(patches[0].body["metadata"]["ownerReferences"][0]["kind"], "Job");
    Ok(())
}

#[tokio::test]
async fn adopting_a_replaced_object_is_refused() -> TestResult {
    let child = receipt(K8sResourceKind::Secret, "token", "child-uid")?;
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, seen) = client(|_, _| {
        (
            200,
            object(
                K8sResourceKind::Secret,
                "token",
                Some("other-uid"),
                Some("9"),
                json!([]),
            ),
        )
    });

    let result = set_exact_resource_owner(&client, &child, &owner).await;

    assert!(matches!(result, Err(K8sOwnershipError::ReplacedBeforeOwnership)));
    assert!(only(&seen, &Method::PATCH).is_empty());
    Ok(())
}

#[tokio::test]
async fn adopting_an_object_without_a_revision_is_refused() -> TestResult {
    let child = receipt(K8sResourceKind::Secret, "token", "child-uid")?;
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, _) = client(|_, _| {
        (
            200,
            object(K8sResourceKind::Secret, "token", Some("child-uid"), None, json!([])),
        )
    });

    let result = set_exact_resource_owner(&client, &child, &owner).await;

    assert!(matches!(result, Err(K8sOwnershipError::OwnershipRevisionMissing)));
    Ok(())
}

#[tokio::test]
async fn adopting_a_missing_object_surfaces_the_api_failure() -> TestResult {
    let child = receipt(K8sResourceKind::Secret, "token", "child-uid")?;
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, _) = client(|_, _| failure(404, "NotFound"));

    let result = set_exact_resource_owner(&client, &child, &owner).await;

    assert!(matches!(result, Err(K8sOwnershipError::Kube(_))));
    Ok(())
}

fn secret(name: Option<&str>) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: name.map(str::to_string),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn a_secret_without_a_name_cannot_be_stored() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, _) = client(|_, _| failure(500, "InternalError"));

    let result = put_exact_owned_secret(&client, &owner, secret(None)).await;

    assert!(matches!(result, Err(K8sOwnershipError::OwnedSecretNameMissing)));
    Ok(())
}

#[tokio::test]
async fn a_new_secret_is_created() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let item = object_path(K8sResourceKind::Secret, "request");
    let collection = collection_path(K8sResourceKind::Secret);
    let (client, seen) = client(move |method, requested| match *method {
        Method::POST if requested == collection => (
            201,
            object(
                K8sResourceKind::Secret,
                "request",
                Some("new-uid"),
                Some("1"),
                json!([]),
            ),
        ),
        Method::GET if requested == item => failure(404, "NotFound"),
        _ => failure(500, "InternalError"),
    });

    let stored = put_exact_owned_secret(&client, &owner, secret(Some("request"))).await?;

    assert_eq!(stored.metadata.uid.as_deref(), Some("new-uid"));
    assert_eq!(only(&seen, &Method::POST).len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_secret_the_owner_already_owns_is_replaced_at_its_current_revision() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let item = object_path(K8sResourceKind::Secret, "request");
    let owners = owner_of(&owner);
    let (client, seen) = client(move |method, requested| match *method {
        Method::GET if requested == item => (
            200,
            object(
                K8sResourceKind::Secret,
                "request",
                Some("s-uid"),
                Some("4"),
                owners.clone(),
            ),
        ),
        Method::PUT if requested == item => (
            200,
            object(
                K8sResourceKind::Secret,
                "request",
                Some("s-uid"),
                Some("5"),
                owners.clone(),
            ),
        ),
        _ => failure(500, "InternalError"),
    });

    let stored = put_exact_owned_secret(&client, &owner, secret(Some("request"))).await?;

    assert_eq!(stored.metadata.resource_version.as_deref(), Some("5"));
    let puts = only(&seen, &Method::PUT);
    assert_eq!(puts[0].body["metadata"]["uid"], "s-uid");
    assert_eq!(puts[0].body["metadata"]["resourceVersion"], "4");
    Ok(())
}

#[tokio::test]
async fn a_secret_owned_by_someone_else_is_not_overwritten() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, seen) = client(|_, _| {
        (
            200,
            object(K8sResourceKind::Secret, "request", Some("s-uid"), Some("4"), json!([])),
        )
    });

    let result = put_exact_owned_secret(&client, &owner, secret(Some("request"))).await;

    assert!(matches!(result, Err(K8sOwnershipError::StoredRequestForeign)));
    assert!(only(&seen, &Method::PUT).is_empty());
    Ok(())
}

#[tokio::test]
async fn an_owned_secret_without_a_uid_or_revision_is_not_replaced() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let owners = owner_of(&owner);
    let without_uid = owners.clone();
    let (client, _) = client(move |_, _| {
        (
            200,
            object(K8sResourceKind::Secret, "request", None, Some("4"), without_uid.clone()),
        )
    });
    let result = put_exact_owned_secret(&client, &owner, secret(Some("request"))).await;
    assert!(matches!(result, Err(K8sOwnershipError::StoredRequestUidMissing)));

    let (client, _) = client_without_revision(owners);
    let result = put_exact_owned_secret(&client, &owner, secret(Some("request"))).await;
    assert!(matches!(result, Err(K8sOwnershipError::StoredRequestRevisionMissing)));
    Ok(())
}

fn client_without_revision(owners: Value) -> (Client, Arc<Mutex<Vec<Seen>>>) {
    client(move |_, _| {
        (
            200,
            object(K8sResourceKind::Secret, "request", Some("s-uid"), None, owners.clone()),
        )
    })
}

fn policy(name: Option<&str>) -> NetworkPolicy {
    NetworkPolicy {
        metadata: ObjectMeta {
            name: name.map(str::to_string),
            namespace: Some(NAMESPACE.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn removing_the_policy_deletes_the_previous_one() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let previous = receipt(K8sResourceKind::NetworkPolicy, "run-policy", "np-uid")?;
    let (client, seen) = client(|method, _| match *method {
        Method::DELETE => (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"})),
        _ => failure(404, "NotFound"),
    });

    let applied = apply_exact_network_policy(&client, &owner, Some(&previous), None).await?;

    assert!(applied.is_none());
    assert_eq!(only(&seen, &Method::DELETE).len(), 1);
    Ok(())
}

#[tokio::test]
async fn removing_a_policy_that_was_never_applied_does_nothing() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, seen) = client(|_, _| failure(500, "InternalError"));

    let applied = apply_exact_network_policy(&client, &owner, None, None).await?;

    assert!(applied.is_none());
    assert!(locked(&seen).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_policy_without_a_name_is_refused() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let (client, _) = client(|_, _| failure(500, "InternalError"));

    let result = apply_exact_network_policy(&client, &owner, None, Some(policy(None))).await;

    assert!(matches!(result, Err(K8sOwnershipError::NetworkPolicyNameMissing)));
    Ok(())
}

#[tokio::test]
async fn a_new_policy_is_created() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let collection = collection_path(K8sResourceKind::NetworkPolicy);
    let (client, _) = client(move |method, requested| match *method {
        Method::POST if requested == collection => (
            201,
            object(
                K8sResourceKind::NetworkPolicy,
                "run-policy",
                Some("np-uid"),
                Some("1"),
                json!([]),
            ),
        ),
        _ => failure(404, "NotFound"),
    });

    let applied = apply_exact_network_policy(&client, &owner, None, Some(policy(Some("run-policy")))).await?;

    assert_eq!(
        applied.and_then(|applied| applied.metadata.uid),
        Some("np-uid".to_string())
    );
    Ok(())
}

#[tokio::test]
async fn a_held_and_owned_policy_is_replaced_at_its_current_revision() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let previous = receipt(K8sResourceKind::NetworkPolicy, "run-policy", "np-uid")?;
    let item = object_path(K8sResourceKind::NetworkPolicy, "run-policy");
    let owners = owner_of(&owner);
    let (client, seen) = client(move |method, requested| match *method {
        Method::GET if requested == item => (
            200,
            object(
                K8sResourceKind::NetworkPolicy,
                "run-policy",
                Some("np-uid"),
                Some("3"),
                owners.clone(),
            ),
        ),
        Method::PUT if requested == item => (
            200,
            object(
                K8sResourceKind::NetworkPolicy,
                "run-policy",
                Some("np-uid"),
                Some("4"),
                owners.clone(),
            ),
        ),
        _ => failure(500, "InternalError"),
    });

    let applied =
        apply_exact_network_policy(&client, &owner, Some(&previous), Some(policy(Some("run-policy")))).await?;

    assert_eq!(
        applied.and_then(|applied| applied.metadata.resource_version),
        Some("4".to_string())
    );
    let puts = only(&seen, &Method::PUT);
    assert_eq!(puts[0].body["metadata"]["uid"], "np-uid");
    assert_eq!(puts[0].body["metadata"]["resourceVersion"], "3");
    Ok(())
}

#[tokio::test]
async fn a_policy_that_is_not_held_is_not_replaced() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let owners = owner_of(&owner);
    let held_elsewhere = receipt(K8sResourceKind::NetworkPolicy, "run-policy", "other-uid")?;
    let existing = move |owners: Value| {
        client(move |_, _| {
            (
                200,
                object(
                    K8sResourceKind::NetworkPolicy,
                    "run-policy",
                    Some("np-uid"),
                    Some("3"),
                    owners.clone(),
                ),
            )
        })
    };

    let (client, _) = existing(owners.clone());
    let result = apply_exact_network_policy(&client, &owner, None, Some(policy(Some("run-policy")))).await;
    assert!(matches!(result, Err(K8sOwnershipError::NetworkPolicyNotHeld)));

    let (client, _) = existing(owners);
    let result =
        apply_exact_network_policy(&client, &owner, Some(&held_elsewhere), Some(policy(Some("run-policy")))).await;
    assert!(matches!(result, Err(K8sOwnershipError::NetworkPolicyNotHeld)));

    let previous = receipt(K8sResourceKind::NetworkPolicy, "run-policy", "np-uid")?;
    let (client, _) = existing(json!([]));
    let result = apply_exact_network_policy(&client, &owner, Some(&previous), Some(policy(Some("run-policy")))).await;
    assert!(matches!(result, Err(K8sOwnershipError::NetworkPolicyNotHeld)));
    Ok(())
}

#[tokio::test]
async fn a_held_policy_without_a_revision_is_not_replaced() -> TestResult {
    let owner = receipt(K8sResourceKind::Job, "run", "owner-uid")?;
    let previous = receipt(K8sResourceKind::NetworkPolicy, "run-policy", "np-uid")?;
    let owners = owner_of(&owner);
    let (client, _) = client(move |_, _| {
        (
            200,
            object(
                K8sResourceKind::NetworkPolicy,
                "run-policy",
                Some("np-uid"),
                None,
                owners.clone(),
            ),
        )
    });

    let result = apply_exact_network_policy(&client, &owner, Some(&previous), Some(policy(Some("run-policy")))).await;

    assert!(matches!(result, Err(K8sOwnershipError::NetworkPolicyRevisionMissing)));
    Ok(())
}
