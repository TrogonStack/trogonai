mod fake_cluster;

use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde_json::{Value, json};
use trogon_sandbox_contract::NetworkPolicy;

use self::fake_cluster::FakeCluster;
use crate::{
    K8sMicroVmConfig, K8sMicroVmProvider, K8sOwnershipError, K8sProviderError, K8sResourceKind, K8sResourceReceipt,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PODS: &str = "/api/v1/namespaces/work/pods";
const JOBS: &str = "/apis/batch/v1/namespaces/work/jobs";
const SECRETS: &str = "/api/v1/namespaces/work/secrets";
const POLICIES: &str = "/apis/networking.k8s.io/v1/namespaces/work/networkpolicies";
const CONFIG_MAPS: &str = "/api/v1/namespaces/work/configmaps";

const WORKER: &str = "sandbox.trogon.live/worker-identity";
const LEASE: &str = "sandbox.trogon.live/lease-id";
const WARM: &str = "sandbox.trogon.live/warm-pool";
const DEADLINE: &str = "sandbox.trogon.live/lease-deadline";

fn provider(cluster: &FakeCluster) -> K8sMicroVmProvider {
    K8sMicroVmProvider::with_client(
        cluster.client(),
        K8sMicroVmConfig::new("work").with_worker_identity("w1"),
    )
}

fn meta(name: &str, uid: Option<&str>, revision: Option<&str>, labels: Value, owners: Value) -> Value {
    let mut metadata = json!({
        "name": name,
        "namespace": "work",
        "labels": labels,
        "ownerReferences": owners,
        "creationTimestamp": "2025-12-31T00:00:00Z",
    });
    if let Some(uid) = uid {
        metadata["uid"] = json!(uid);
    }
    if let Some(revision) = revision {
        metadata["resourceVersion"] = json!(revision);
    }
    metadata
}

fn owner_ref(kind: &str, name: &str, uid: &str) -> Value {
    let api_version = if kind == "Job" { "batch/v1" } else { "v1" };
    json!([{"apiVersion": api_version, "kind": kind, "name": name, "uid": uid}])
}

fn secret(name: &str, uid: &str, owners: Value) -> Value {
    json!({"apiVersion": "v1", "kind": "Secret", "metadata": meta(name, Some(uid), Some("1"), json!({}), owners)})
}

fn plain(kind: &str, name: &str, uid: &str, owners: Value) -> Value {
    let api_version = if kind == "NetworkPolicy" {
        "networking.k8s.io/v1"
    } else {
        "v1"
    };
    let mut object = json!({"apiVersion": api_version, "kind": kind, "metadata": meta(name, Some(uid), Some("1"), json!({}), owners)});
    if kind == "NetworkPolicy" {
        object["spec"] = json!({"podSelector": {}});
    }
    object
}

fn job(name: &str, uid: &str, labels: Value) -> Value {
    json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": meta(name, Some(uid), Some("1"), labels, json!([])),
        "spec": {"template": {"spec": {"containers": [{"name": "agent"}]}}},
    })
}

fn pod(name: &str, uid: &str, labels: Value, owners: Value, phase: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": meta(name, Some(uid), Some("1"), labels, owners),
        "spec": {"containers": [{"name": "agent"}]},
        "status": {
            "phase": phase,
            "containerStatuses": [{
                "name": "agent", "image": "image", "imageID": "", "ready": phase == "Running",
                "restartCount": 0, "state": if phase == "Running" { json!({"running": {}}) } else { json!({"waiting": {}}) },
            }],
        },
    })
}

fn pod_with_restore_secret(mut object: Value, secret: &str) -> Value {
    object["spec"]["volumes"] = json!([{"name": "restore-data", "secret": {"secretName": secret}}]);
    object
}

fn receipt(kind: K8sResourceKind, name: &str, uid: &str) -> Result<K8sResourceReceipt, K8sOwnershipError> {
    K8sResourceReceipt::from_metadata(
        kind,
        "work",
        &ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some("work".to_string()),
            uid: Some(uid.to_string()),
            ..Default::default()
        },
    )
}

fn pod_model(value: Value) -> Result<Pod, serde_json::Error> {
    serde_json::from_value(value)
}

#[tokio::test]
async fn the_provider_reports_the_worker_identity_it_was_configured_with() {
    let cluster = FakeCluster::default();

    assert_eq!(provider(&cluster).worker_identity(), Some("w1"));
    let anonymous = K8sMicroVmProvider::with_client(cluster.client(), K8sMicroVmConfig::new("work"));
    assert_eq!(anonymous.worker_identity(), None);
}

#[tokio::test]
async fn receipts_are_only_collected_by_a_scoped_provider() -> TestResult {
    let cluster = FakeCluster::default();
    let plain_provider = provider(&cluster);
    let created = serde_json::from_value::<ObjectMeta>(meta("a", Some("u1"), None, json!({}), json!([])))?;

    plain_provider.record_created_resource(K8sResourceKind::Secret, &created)?;
    assert!(plain_provider.resource_receipts().is_empty());

    let scoped = plain_provider.with_resource_receipts();
    scoped.record_created_resource(K8sResourceKind::Secret, &created)?;
    scoped.record_created_resource(K8sResourceKind::Secret, &created)?;
    assert_eq!(
        scoped.resource_receipts(),
        vec![receipt(K8sResourceKind::Secret, "a", "u1")?]
    );
    assert!(plain_provider.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_created_resource_without_a_uid_is_not_recorded() -> TestResult {
    let cluster = FakeCluster::default();
    let scoped = provider(&cluster).with_resource_receipts();
    let created = serde_json::from_value::<ObjectMeta>(meta("a", None, None, json!({}), json!([])))?;

    let result = scoped.record_created_resource(K8sResourceKind::Secret, &created);

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::UidNotConfirmed))
    ));
    assert!(scoped.resource_receipts().is_empty());
    Ok(())
}

#[tokio::test]
async fn recorded_resources_are_attached_to_their_owner() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(SECRETS, secret("run-token", "secret-uid", json!([])));
    let scoped = provider(&cluster).with_resource_receipts();
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    for (kind, name, uid) in [
        (K8sResourceKind::Job, "run", "job-uid"),
        (K8sResourceKind::Secret, "run-token", "secret-uid"),
    ] {
        let metadata = serde_json::from_value::<ObjectMeta>(meta(name, Some(uid), None, json!({}), json!([])))?;
        scoped.record_created_resource(kind, &metadata)?;
    }

    scoped.attach_recorded_resources(&owner).await?;

    let adopted = cluster.get(&format!("{SECRETS}/run-token")).unwrap_or_default();
    assert_eq!(adopted["metadata"]["ownerReferences"][0]["uid"], "job-uid");
    Ok(())
}

#[tokio::test]
async fn attaching_reports_a_resource_that_vanished() -> TestResult {
    let cluster = FakeCluster::default();
    let scoped = provider(&cluster).with_resource_receipts();
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    let metadata = serde_json::from_value::<ObjectMeta>(meta("gone", Some("secret-uid"), None, json!({}), json!([])))?;
    scoped.record_created_resource(K8sResourceKind::Secret, &metadata)?;

    let result = scoped.attach_recorded_resources(&owner).await;

    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn releasing_deletes_every_resource() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(SECRETS, secret("a", "ua", json!([])));
    cluster.put(SECRETS, secret("b", "ub", json!([])));
    let resources = [
        receipt(K8sResourceKind::Secret, "a", "ua")?,
        receipt(K8sResourceKind::Secret, "b", "ub")?,
    ];

    provider(&cluster).release_resources(&resources).await?;

    assert!(cluster.get(&format!("{SECRETS}/a")).is_none());
    assert!(cluster.get(&format!("{SECRETS}/b")).is_none());
    Ok(())
}

#[tokio::test]
async fn releasing_attempts_everything_and_reports_the_first_failure() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(SECRETS, secret("a", "ua", json!([])));
    cluster.put(SECRETS, secret("b", "ub", json!([])));
    cluster.put(SECRETS, secret("c", "uc", json!([])));
    cluster.fail_deletes_of(&format!("{SECRETS}/a"));
    cluster.fail_deletes_of(&format!("{SECRETS}/b"));
    let resources = [
        receipt(K8sResourceKind::Secret, "a", "ua")?,
        receipt(K8sResourceKind::Secret, "b", "ub")?,
        receipt(K8sResourceKind::Secret, "c", "uc")?,
    ];

    let result = provider(&cluster).release_resources(&resources).await;

    assert!(matches!(result, Err(K8sProviderError::Ops(_))));
    assert!(cluster.get(&format!("{SECRETS}/c")).is_none());
    Ok(())
}

#[tokio::test]
async fn a_resource_is_held_only_while_the_same_incarnation_exists() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(PODS, pod("agent", "u1", json!({}), json!([]), "Running"));
    let provider = provider(&cluster);

    assert!(
        provider
            .resource_is_held(&receipt(K8sResourceKind::Pod, "agent", "u1")?)
            .await?
    );
    assert!(
        !provider
            .resource_is_held(&receipt(K8sResourceKind::Pod, "agent", "replaced")?)
            .await?
    );
    assert!(
        !provider
            .resource_is_held(&receipt(K8sResourceKind::Pod, "absent", "u1")?)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn an_object_without_a_confirmed_uid_is_not_a_held_resource() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(
        PODS,
        json!({"apiVersion": "v1", "kind": "Pod", "metadata": meta("agent", None, Some("1"), json!({}), json!([]))}),
    );

    let result = provider(&cluster)
        .resource_is_held(&receipt(K8sResourceKind::Pod, "agent", "u1")?)
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::UidNotConfirmed))
    ));
    Ok(())
}

#[tokio::test]
async fn the_resources_a_job_owns_are_found_by_their_derived_names() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    let owners = owner_ref("Job", "run", "job-uid");
    cluster.put(SECRETS, secret("run-clone-creds", "s1", owners.clone()));
    cluster.put(SECRETS, secret("run-restore", "s2", json!([])));
    cluster.put(SECRETS, secret("run-lease-req", "s3", owners.clone()));
    cluster.put(POLICIES, plain("NetworkPolicy", "run-netpol", "p1", owners.clone()));
    cluster.put(CONFIG_MAPS, plain("ConfigMap", "run-egress-policy", "c1", owners));

    let owned = provider(&cluster).owned_resource_receipts(&owner).await?;

    let names: Vec<&str> = owned.iter().map(K8sResourceReceipt::name).collect();
    assert_eq!(
        names,
        ["run-clone-creds", "run-lease-req", "run-netpol", "run-egress-policy"]
    );
    Ok(())
}

#[tokio::test]
async fn a_pod_owns_only_its_request_secret_and_policy_objects() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Pod, "warm-1", "pod-uid")?;
    cluster.put(
        SECRETS,
        secret("warm-1-lease-req", "s1", owner_ref("Pod", "warm-1", "pod-uid")),
    );
    cluster.put(
        SECRETS,
        secret("warm-1-clone-creds", "s2", owner_ref("Pod", "warm-1", "pod-uid")),
    );

    let owned = provider(&cluster).owned_resource_receipts(&owner).await?;

    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].name(), "warm-1-lease-req");
    Ok(())
}

#[tokio::test]
async fn an_owned_object_with_a_broken_identity_fails_the_listing() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Pod, "warm-1", "pod-uid")?;
    cluster.put(
        SECRETS,
        json!({"apiVersion": "v1", "kind": "Secret", "metadata": meta(
            "warm-1-lease-req", None, Some("1"), json!({}), owner_ref("Pod", "warm-1", "pod-uid"))}),
    );

    let result = provider(&cluster).owned_resource_receipts(&owner).await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::UidNotConfirmed))
    ));
    Ok(())
}

#[tokio::test]
async fn an_owned_restore_secret_is_recorded() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Pod, "warm-1", "pod-uid")?;
    cluster.put(
        SECRETS,
        secret("restore-1", "s1", owner_ref("Pod", "warm-1", "pod-uid")),
    );
    let scoped = provider(&cluster).with_resource_receipts();

    scoped.record_owned_secret(&owner, "restore-1").await?;

    assert_eq!(
        scoped.resource_receipts(),
        vec![receipt(K8sResourceKind::Secret, "restore-1", "s1")?]
    );
    Ok(())
}

#[tokio::test]
async fn a_restore_secret_that_is_missing_or_foreign_is_refused() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Pod, "warm-1", "pod-uid")?;
    cluster.put(SECRETS, secret("restore-2", "s2", json!([])));
    let provider = provider(&cluster);

    let missing = provider.record_owned_secret(&owner, "restore-1").await;
    let foreign = provider.record_owned_secret(&owner, "restore-2").await;

    assert!(matches!(
        missing,
        Err(K8sProviderError::Ownership(
            K8sOwnershipError::RestoreSecretNotConfirmed
        ))
    ));
    assert!(matches!(
        foreign,
        Err(K8sProviderError::Ownership(K8sOwnershipError::RestoreSecretNotOwned))
    ));
    Ok(())
}

#[tokio::test]
async fn a_restore_secret_lookup_failure_is_reported() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Pod, "warm-1", "pod-uid")?;
    cluster.put(
        SECRETS,
        json!({"apiVersion": "v1", "kind": "Secret", "metadata": meta("restore-1", None, Some("1"), json!({}), json!([]))}),
    );

    let result = provider(&cluster).record_owned_secret(&owner, "restore-1").await;

    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn the_lease_request_is_stored_under_its_owner_and_recorded() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    let scoped = provider(&cluster).with_resource_receipts();

    let stored = scoped
        .store_owned_lease_request(&owner, "lease-9", b"{\"a\":1}")
        .await?;

    assert_eq!(stored.name(), "run-lease-req");
    assert_eq!(scoped.resource_receipts(), vec![stored]);
    let object = cluster.get(&format!("{SECRETS}/run-lease-req")).unwrap_or_default();
    assert_eq!(object["metadata"]["labels"][LEASE], "lease-9");
    assert_eq!(object["metadata"]["ownerReferences"][0]["uid"], "job-uid");
    Ok(())
}

#[tokio::test]
async fn a_lease_request_for_a_replaced_owner_is_not_stored() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "replacement", json!({})));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .store_owned_lease_request(&owner, "lease-9", b"{}")
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::RequestOwnerReplaced))
    ));
    assert!(cluster.get(&format!("{SECRETS}/run-lease-req")).is_none());
    Ok(())
}

#[tokio::test]
async fn a_created_lease_request_the_cluster_did_not_identify_is_reported() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.anonymous_creates();
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .store_owned_lease_request(&owner, "lease-9", b"{}")
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::UidNotConfirmed))
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_request_secret_the_cluster_rejects_is_reported() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(SECRETS, secret("run-lease-req", "s1", json!([])));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .store_owned_lease_request(&owner, "lease-9", b"{}")
        .await;

    assert!(matches!(result, Err(K8sProviderError::Ops(_))));
    Ok(())
}

#[tokio::test]
async fn a_deny_all_policy_is_created_under_its_owner() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let applied = provider(&cluster)
        .push_owned_network_policy(&owner, None, &NetworkPolicy::DenyAll, &BTreeMap::new())
        .await?;

    let applied = applied.ok_or("a deny-all policy is applied")?;
    assert_eq!(applied.kind(), K8sResourceKind::NetworkPolicy);
    let object = cluster.get(&format!("{POLICIES}/run-netpol")).unwrap_or_default();
    assert_eq!(object["metadata"]["ownerReferences"][0]["uid"], "job-uid");
    Ok(())
}

#[tokio::test]
async fn allowing_all_egress_removes_the_previous_policy() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(
        POLICIES,
        plain(
            "NetworkPolicy",
            "run-netpol",
            "np-uid",
            owner_ref("Job", "run", "job-uid"),
        ),
    );
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    let previous = receipt(K8sResourceKind::NetworkPolicy, "run-netpol", "np-uid")?;

    let applied = provider(&cluster)
        .push_owned_network_policy(&owner, Some(&previous), &NetworkPolicy::AllowAll, &BTreeMap::new())
        .await?;

    assert!(applied.is_none());
    assert!(cluster.get(&format!("{POLICIES}/run-netpol")).is_none());
    Ok(())
}

#[tokio::test]
async fn a_policy_for_a_replaced_owner_is_not_applied() -> TestResult {
    let cluster = FakeCluster::default();
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .push_owned_network_policy(&owner, None, &NetworkPolicy::DenyAll, &BTreeMap::new())
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(
            K8sOwnershipError::NetworkPolicyOwnerReplaced
        ))
    ));
    Ok(())
}

fn idle_pod(name: &str, uid: &str) -> Value {
    pod(name, uid, json!({WARM: "idle"}), json!([]), "Running")
}

#[tokio::test]
async fn an_idle_warm_pod_is_evicted_with_its_restore_secret() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = pod_with_restore_secret(idle_pod("warm-1", "pod-uid"), "restore-1");
    cluster.put(PODS, observed.clone());
    cluster.put(
        SECRETS,
        secret("restore-1", "s1", owner_ref("Pod", "warm-1", "pod-uid")),
    );
    let scoped = provider(&cluster).with_resource_receipts();

    let evicted = scoped.delete_idle_warm_pod(&pod_model(observed)?).await?;

    assert!(evicted);
    assert!(cluster.get(&format!("{PODS}/warm-1")).is_none());
    assert!(cluster.get(&format!("{SECRETS}/restore-1")).is_none());
    assert_eq!(
        scoped.resource_receipts(),
        vec![receipt(K8sResourceKind::Secret, "restore-1", "s1")?]
    );
    Ok(())
}

#[tokio::test]
async fn an_idle_warm_pod_without_a_restore_secret_is_evicted_alone() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = idle_pod("warm-1", "pod-uid");
    cluster.put(PODS, observed.clone());

    let evicted = provider(&cluster).delete_idle_warm_pod(&pod_model(observed)?).await?;

    assert!(evicted);
    assert!(cluster.get(&format!("{PODS}/warm-1")).is_none());
    Ok(())
}

#[tokio::test]
async fn a_restore_secret_the_pod_does_not_own_is_left_alone() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = pod_with_restore_secret(idle_pod("warm-1", "pod-uid"), "restore-1");
    cluster.put(PODS, observed.clone());
    cluster.put(SECRETS, secret("restore-1", "s1", json!([])));

    let evicted = provider(&cluster).delete_idle_warm_pod(&pod_model(observed)?).await?;

    assert!(evicted);
    assert!(cluster.get(&format!("{SECRETS}/restore-1")).is_some());
    Ok(())
}

#[tokio::test]
async fn only_a_pod_observed_idle_can_be_evicted() -> TestResult {
    let cluster = FakeCluster::default();
    let provider = provider(&cluster);
    let claimed = pod("warm-1", "pod-uid", json!({WARM: "claimed"}), json!([]), "Running");
    let unlabeled = pod("warm-2", "pod-uid", json!({}), json!([]), "Running");

    for observed in [claimed, unlabeled] {
        let result = provider.delete_idle_warm_pod(&pod_model(observed)?).await;
        assert!(matches!(
            result,
            Err(K8sProviderError::Ownership(K8sOwnershipError::WarmEvictionNotIdle))
        ));
    }
    Ok(())
}

#[tokio::test]
async fn an_idle_pod_without_a_confirmed_identity_or_revision_is_not_evicted() -> TestResult {
    let cluster = FakeCluster::default();
    let provider = provider(&cluster);
    let mut no_uid = idle_pod("warm-1", "pod-uid");
    no_uid["metadata"]["uid"] = Value::Null;
    let mut no_revision = idle_pod("warm-2", "pod-uid");
    no_revision["metadata"]["resourceVersion"] = Value::Null;

    let unidentified = provider.delete_idle_warm_pod(&pod_model(no_uid)?).await;
    let unfenced = provider.delete_idle_warm_pod(&pod_model(no_revision)?).await;

    assert!(matches!(
        unidentified,
        Err(K8sProviderError::Ownership(K8sOwnershipError::UidNotConfirmed))
    ));
    assert!(matches!(
        unfenced,
        Err(K8sProviderError::Ownership(K8sOwnershipError::WarmPodRevisionMissing))
    ));
    Ok(())
}

#[tokio::test]
async fn a_pod_that_changed_since_it_was_observed_is_reported_as_not_evicted() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = idle_pod("warm-1", "pod-uid");
    let mut current = observed.clone();
    current["metadata"]["resourceVersion"] = json!("7");
    cluster.put(PODS, current);

    let evicted = provider(&cluster).delete_idle_warm_pod(&pod_model(observed)?).await?;

    assert!(!evicted);
    assert!(cluster.get(&format!("{PODS}/warm-1")).is_some());
    Ok(())
}

#[tokio::test]
async fn a_failed_eviction_of_an_unchanged_pod_is_an_error() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = idle_pod("warm-1", "pod-uid");
    cluster.put(PODS, observed.clone());
    cluster.fail_deletes_of(&format!("{PODS}/warm-1"));

    let result = provider(&cluster).delete_idle_warm_pod(&pod_model(observed)?).await;

    assert!(matches!(result, Err(K8sProviderError::Ops(_))));
    Ok(())
}

#[tokio::test]
async fn an_eviction_whose_reinspection_fails_reports_that_failure() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = idle_pod("warm-1", "pod-uid");
    let mut broken = observed.clone();
    broken["metadata"]["uid"] = Value::Null;
    cluster.put(PODS, broken);
    cluster.fail_deletes_of(&format!("{PODS}/warm-1"));

    let result = provider(&cluster).delete_idle_warm_pod(&pod_model(observed)?).await;

    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_restore_secret_that_cannot_be_deleted_fails_the_eviction() -> TestResult {
    let cluster = FakeCluster::default();
    let observed = pod_with_restore_secret(idle_pod("warm-1", "pod-uid"), "restore-1");
    cluster.put(PODS, observed.clone());
    cluster.put(
        SECRETS,
        secret("restore-1", "s1", owner_ref("Pod", "warm-1", "pod-uid")),
    );
    cluster.fail_deletes_of(&format!("{SECRETS}/restore-1"));

    let result = provider(&cluster).delete_idle_warm_pod(&pod_model(observed)?).await;

    assert!(result.is_err());
    Ok(())
}

fn owned_pod(name: &str, uid: &str, phase: &str) -> Value {
    pod(
        name,
        uid,
        json!({"job-name": "run"}),
        owner_ref("Job", "run", "job-uid"),
        phase,
    )
}

#[tokio::test(start_paused = true)]
async fn the_ready_pod_of_the_job_is_returned() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(
        PODS,
        pod("a-stranger", "p0", json!({"job-name": "run"}), json!([]), "Running"),
    );
    cluster.put(PODS, owned_pod("run-abc", "p1", "Running"));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    let scoped = provider(&cluster).with_resource_receipts();

    let ready = scoped.wait_for_owned_job_pod(&owner, Duration::from_secs(30)).await?;

    assert_eq!(ready.name(), "run-abc");
    assert_eq!(
        scoped.resource_receipts(),
        vec![receipt(K8sResourceKind::Pod, "run-abc", "p1")?]
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_polls_until_the_pod_becomes_ready() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(PODS, owned_pod("run-abc", "p1", "Pending"));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;
    let provider = provider(&cluster);

    let waiting = provider.wait_for_owned_job_pod(&owner, Duration::from_secs(30));
    let starting = async {
        tokio::time::sleep(Duration::from_millis(2500)).await;
        cluster.put(PODS, owned_pod("run-abc", "p1", "Running"));
    };
    let (ready, ()) = tokio::join!(waiting, starting);

    assert_eq!(ready?.name(), "run-abc");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_pod_that_never_starts_times_out() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(PODS, owned_pod("run-abc", "p1", "Pending"));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .wait_for_owned_job_pod(&owner, Duration::from_secs(3))
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::PodStartTimeout { ref job, seconds: 3 })) if job == "run"
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_pod_that_ended_before_it_was_ready_fails_the_wait() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    cluster.put(PODS, owned_pod("run-abc", "p1", "Failed"));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .wait_for_owned_job_pod(&owner, Duration::from_secs(30))
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::PodFailedToStart(ref reason))) if reason.contains("Failed")
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_refuses_a_job_outside_the_namespace_or_of_another_kind() -> TestResult {
    let cluster = FakeCluster::default();
    let provider = provider(&cluster);
    let pod_owner = receipt(K8sResourceKind::Pod, "run", "p1")?;
    let elsewhere = K8sResourceReceipt::from_metadata(
        K8sResourceKind::Job,
        "elsewhere",
        &ObjectMeta {
            name: Some("run".into()),
            namespace: Some("elsewhere".into()),
            uid: Some("job-uid".into()),
            ..Default::default()
        },
    )?;

    for owner in [pod_owner, elsewhere] {
        let result = provider.wait_for_owned_job_pod(&owner, Duration::from_secs(1)).await;
        assert!(matches!(
            result,
            Err(K8sProviderError::Ownership(K8sOwnershipError::JobOutsideNamespace))
        ));
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_stops_when_the_job_was_replaced() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "someone-else", json!({})));
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .wait_for_owned_job_pod(&owner, Duration::from_secs(30))
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::JobReplaced))
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_reports_a_pod_the_cluster_did_not_identify() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", json!({})));
    let mut unidentified = owned_pod("run-abc", "p1", "Running");
    unidentified["metadata"]["uid"] = Value::Null;
    cluster.put(PODS, unidentified);
    let owner = receipt(K8sResourceKind::Job, "run", "job-uid")?;

    let result = provider(&cluster)
        .wait_for_owned_job_pod(&owner, Duration::from_secs(30))
        .await;

    assert!(matches!(
        result,
        Err(K8sProviderError::Ownership(K8sOwnershipError::UidNotConfirmed))
    ));
    Ok(())
}

fn lease_labels(identity: &str, lease: &str) -> Value {
    json!({WORKER: identity, LEASE: lease})
}

#[tokio::test]
async fn a_provider_without_a_worker_identity_recovers_nothing() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", lease_labels("w1", "l1")));
    let anonymous = K8sMicroVmProvider::with_client(cluster.client(), K8sMicroVmConfig::new("work"));
    let blank =
        K8sMicroVmProvider::with_client(cluster.client(), K8sMicroVmConfig::new("work").with_worker_identity(""));

    assert!(anonymous.confirmed_lease_inventory().await?.is_empty());
    assert!(blank.confirmed_lease_inventory().await?.is_empty());
    assert!(cluster.requests().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_live_job_lease_is_recovered_with_its_pod_and_owned_objects() -> TestResult {
    let cluster = FakeCluster::default();
    let owners = owner_ref("Job", "run", "job-uid");
    cluster.put(JOBS, job("run", "job-uid", lease_labels("w1", "lease-1")));
    cluster.put(JOBS, job("theirs", "j2", lease_labels("w2", "lease-2")));
    cluster.put(PODS, owned_pod("run-abc", "p1", "Running"));
    cluster.put(
        PODS,
        pod("run-other", "p2", json!({"job-name": "run"}), json!([]), "Running"),
    );
    cluster.put(SECRETS, secret("run-lease-req", "s1", owners));

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    assert_eq!(leases.len(), 1);
    let lease = &leases[0];
    assert_eq!(lease.lease_id, "lease-1");
    assert_eq!(lease.owner, receipt(K8sResourceKind::Job, "run", "job-uid")?);
    assert_eq!(lease.pod, Some(receipt(K8sResourceKind::Pod, "run-abc", "p1")?));
    assert!(!lease.expired);
    let names: Vec<&str> = lease.resources.iter().map(K8sResourceReceipt::name).collect();
    assert_eq!(names, ["run", "run-abc", "run-lease-req"]);
    Ok(())
}

#[tokio::test]
async fn a_job_lease_whose_pod_is_not_ready_has_no_pod() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.put(JOBS, job("run", "job-uid", lease_labels("w1", "lease-1")));
    cluster.put(PODS, owned_pod("run-abc", "p1", "Pending"));
    let mut deleting = owned_pod("run-def", "p2", "Running");
    deleting["metadata"]["deletionTimestamp"] = json!("2025-12-31T23:00:00Z");
    cluster.put(PODS, deleting);
    let mut unidentified = owned_pod("run-ghi", "p3", "Running");
    unidentified["metadata"]["uid"] = Value::Null;
    cluster.put(PODS, unidentified);

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    assert_eq!(leases[0].pod, None);
    assert_eq!(leases[0].resources.len(), 3);
    Ok(())
}

#[tokio::test]
async fn job_leases_are_marked_expired_when_the_job_ended_or_ran_past_its_deadline() -> TestResult {
    let cluster = FakeCluster::default();
    let mut deleting = job("deleting", "j1", lease_labels("w1", "l1"));
    deleting["metadata"]["deletionTimestamp"] = json!("2025-12-31T23:00:00Z");
    let mut finished = job("finished", "j2", lease_labels("w1", "l2"));
    finished["status"] = json!({"conditions": [{"type": "Complete", "status": "True"}]});
    let mut overdue = job("overdue", "j3", lease_labels("w1", "l3"));
    overdue["spec"]["activeDeadlineSeconds"] = json!(60);
    let mut within = job("within", "j4", lease_labels("w1", "l4"));
    within["spec"]["activeDeadlineSeconds"] = json!(1_000_000);
    for object in [deleting, finished, overdue, within] {
        cluster.put(JOBS, object);
    }

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    let expired: BTreeMap<&str, bool> = leases
        .iter()
        .map(|lease| (lease.lease_id.as_str(), lease.expired))
        .collect();
    assert_eq!(
        expired,
        BTreeMap::from([("l1", true), ("l2", true), ("l3", true), ("l4", false)])
    );
    Ok(())
}

#[tokio::test]
async fn jobs_that_do_not_qualify_as_this_workers_lease_are_skipped() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.ignore_selectors();
    let mut unlabeled = job("unlabeled", "j1", json!({}));
    unlabeled["metadata"]["labels"] = Value::Null;
    cluster.put(JOBS, unlabeled);
    cluster.put(JOBS, job("foreign", "j2", lease_labels("w2", "l2")));
    cluster.put(JOBS, job("no-lease", "j3", json!({WORKER: "w1"})));
    let mut unidentified = job("unidentified", "j4", lease_labels("w1", "l4"));
    unidentified["metadata"]["uid"] = Value::Null;
    cluster.put(JOBS, unidentified);

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    assert!(leases.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_claimed_warm_pod_is_recovered_with_its_restore_secret() -> TestResult {
    let cluster = FakeCluster::default();
    let owners = owner_ref("Pod", "warm-1", "pod-uid");
    let labels = json!({WORKER: "w1", LEASE: "lease-1", WARM: "claimed"});
    cluster.put(
        PODS,
        pod_with_restore_secret(pod("warm-1", "pod-uid", labels, json!([]), "Running"), "restore-1"),
    );
    cluster.put(SECRETS, secret("restore-1", "s1", owners.clone()));
    cluster.put(SECRETS, secret("warm-1-lease-req", "s2", owners));

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    assert_eq!(leases.len(), 1);
    let lease = &leases[0];
    assert_eq!(lease.lease_id, "lease-1");
    assert_eq!(lease.pod, Some(receipt(K8sResourceKind::Pod, "warm-1", "pod-uid")?));
    assert!(!lease.expired);
    let names: Vec<&str> = lease.resources.iter().map(K8sResourceReceipt::name).collect();
    assert_eq!(names, ["warm-1", "warm-1-lease-req", "restore-1"]);
    Ok(())
}

#[tokio::test]
async fn a_claimed_warm_pod_that_is_past_its_deadline_or_deleting_is_expired() -> TestResult {
    let cluster = FakeCluster::default();
    let claimed = |lease: &str, extra: Value| {
        let mut labels = json!({WORKER: "w1", LEASE: lease, WARM: "claimed"});
        if let Value::Object(extra) = extra {
            for (key, value) in extra {
                labels[key] = value;
            }
        }
        labels
    };
    cluster.put(
        PODS,
        pod(
            "late",
            "u1",
            claimed("l1", json!({DEADLINE: "1767225000"})),
            json!([]),
            "Running",
        ),
    );
    cluster.put(
        PODS,
        pod(
            "early",
            "u2",
            claimed("l2", json!({DEADLINE: "1767229999"})),
            json!([]),
            "Running",
        ),
    );
    let mut deleting = pod("deleting", "u3", claimed("l3", json!({})), json!([]), "Running");
    deleting["metadata"]["deletionTimestamp"] = json!("2025-12-31T23:00:00Z");
    cluster.put(PODS, deleting);
    cluster.put(
        PODS,
        pod("starting", "u4", claimed("l4", json!({})), json!([]), "Pending"),
    );

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    let state: BTreeMap<&str, (bool, bool)> = leases
        .iter()
        .map(|lease| (lease.lease_id.as_str(), (lease.expired, lease.pod.is_some())))
        .collect();
    assert_eq!(
        state,
        BTreeMap::from([
            ("l1", (true, true)),
            ("l2", (false, true)),
            ("l3", (true, false)),
            ("l4", (false, false))
        ])
    );
    Ok(())
}

#[tokio::test]
async fn a_restore_secret_another_object_owns_is_not_part_of_the_warm_lease() -> TestResult {
    let cluster = FakeCluster::default();
    let labels = json!({WORKER: "w1", LEASE: "lease-1", WARM: "claimed"});
    cluster.put(
        PODS,
        pod_with_restore_secret(pod("warm-1", "pod-uid", labels, json!([]), "Running"), "restore-1"),
    );
    cluster.put(SECRETS, secret("restore-1", "s1", json!([])));

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    assert_eq!(leases[0].resources.len(), 1);
    Ok(())
}

#[tokio::test]
async fn pods_that_do_not_qualify_as_this_workers_claimed_lease_are_skipped() -> TestResult {
    let cluster = FakeCluster::default();
    cluster.ignore_selectors();
    let mut unlabeled = pod("unlabeled", "u1", json!({}), json!([]), "Running");
    unlabeled["metadata"]["labels"] = Value::Null;
    cluster.put(PODS, unlabeled);
    cluster.put(
        PODS,
        pod(
            "foreign",
            "u2",
            json!({WORKER: "w2", LEASE: "l2", WARM: "claimed"}),
            json!([]),
            "Running",
        ),
    );
    cluster.put(
        PODS,
        pod(
            "idle",
            "u3",
            json!({WORKER: "w1", LEASE: "l3", WARM: "idle"}),
            json!([]),
            "Running",
        ),
    );
    cluster.put(
        PODS,
        pod(
            "no-lease",
            "u4",
            json!({WORKER: "w1", WARM: "claimed"}),
            json!([]),
            "Running",
        ),
    );
    cluster.put(
        PODS,
        pod(
            "unidentified",
            "u5",
            json!({WORKER: "w1", LEASE: "l5", WARM: "claimed"}),
            json!([]),
            "Running",
        ),
    );
    let mut unidentified = pod(
        "blank",
        "u6",
        json!({WORKER: "w1", LEASE: "l6", WARM: "claimed"}),
        json!([]),
        "Running",
    );
    unidentified["metadata"]["uid"] = Value::Null;
    cluster.put(PODS, unidentified);

    let leases = provider(&cluster).confirmed_lease_inventory().await?;

    let recovered: Vec<&str> = leases.iter().map(|lease| lease.lease_id.as_str()).collect();
    assert_eq!(recovered, ["l5"]);
    Ok(())
}

#[tokio::test]
async fn a_failing_listing_fails_the_inventory() {
    let cluster = FakeCluster::default();
    let broken = K8sMicroVmProvider::with_client(
        kube::Client::new(
            tower::service_fn(|_request: http::Request<kube::client::Body>| async {
                Err::<http::Response<kube::client::Body>, std::io::Error>(std::io::Error::other("unreachable"))
            }),
            "work",
        ),
        K8sMicroVmConfig::new("work").with_worker_identity("w1"),
    );

    assert!(broken.confirmed_lease_inventory().await.is_err());
    assert!(cluster.requests().is_empty());
}
