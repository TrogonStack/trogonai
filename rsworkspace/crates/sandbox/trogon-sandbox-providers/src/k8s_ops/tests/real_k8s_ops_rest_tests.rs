use std::collections::BTreeMap;

use k8s_openapi::api::networking::v1::NetworkPolicySpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;

use super::fake_api_server_tests::FakeApiServer;
use super::fake_reply_tests::FakeReply;
use super::test_objects_tests::{config_map, job, network_policy, pod, secret, secret_with};
use crate::K8sOpsError;
use crate::k8s_ops::FencedPatchOutcome;
use crate::k8s_ops::K8sOps;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn create_job_posts_to_the_namespace() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::object(&job("rs-job"))?]);

    let created = api.ops().create_job("work", job("rs-job")).await?;

    assert_eq!(created.metadata.name.as_deref(), Some("rs-job"));
    let seen = api.seen();
    assert_eq!(seen[0].method, "POST");
    assert!(
        seen[0].uri.starts_with("/apis/batch/v1/namespaces/work/jobs"),
        "{}",
        seen[0].uri
    );
    assert!(seen[0].body.contains("rs-job"));
    Ok(())
}

#[tokio::test]
async fn create_job_maps_a_conflict_to_job_already_exists() {
    let api = FakeApiServer::new(vec![FakeReply::failure(409, "AlreadyExists")]);

    let result = api.ops().create_job("work", job("rs-job")).await;

    assert!(
        matches!(&result, Err(K8sOpsError::JobAlreadyExists(message)) if message == "scripted AlreadyExists"),
        "{result:?}"
    );
}

#[tokio::test]
async fn create_job_keeps_every_other_failure_as_a_kube_error() {
    let api = FakeApiServer::new(vec![FakeReply::failure(403, "Forbidden")]);

    let result = api.ops().create_job("work", job("rs-job")).await;

    assert!(
        matches!(result, Err(K8sOpsError::Kube(error)) if matches!(&*error, kube::Error::Api(status) if status.code == 403))
    );
}

#[tokio::test]
async fn the_other_creates_post_their_object() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::object(&pod("p"))?,
        FakeReply::object(&secret("s"))?,
        FakeReply::object(&network_policy("np"))?,
        FakeReply::object(&config_map("cm"))?,
    ]);
    let ops = api.ops();

    ops.create_pod("work", pod("p")).await?;
    ops.create_secret("work", secret("s")).await?;
    ops.create_network_policy("work", network_policy("np")).await?;
    ops.create_config_map("work", config_map("cm")).await?;

    let uris: Vec<_> = api
        .seen()
        .into_iter()
        .map(|request| (request.method, request.uri))
        .collect();
    assert_eq!(
        uris,
        vec![
            ("POST".to_string(), "/api/v1/namespaces/work/pods?".to_string()),
            ("POST".to_string(), "/api/v1/namespaces/work/secrets?".to_string()),
            (
                "POST".to_string(),
                "/apis/networking.k8s.io/v1/namespaces/work/networkpolicies?".to_string()
            ),
            ("POST".to_string(), "/api/v1/namespaces/work/configmaps?".to_string()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn put_secret_replaces_an_existing_secret() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::object(&secret("s"))?]);

    api.ops().put_secret("work", secret("s")).await?;

    let seen = api.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "PUT");
    assert!(
        seen[0].uri.starts_with("/api/v1/namespaces/work/secrets/s"),
        "{}",
        seen[0].uri
    );
    Ok(())
}

#[tokio::test]
async fn put_secret_creates_a_missing_secret() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::failure(404, "NotFound"),
        FakeReply::object(&secret("s"))?,
    ]);

    api.ops().put_secret("work", secret("s")).await?;

    let methods: Vec<_> = api.seen().into_iter().map(|request| request.method).collect();
    assert_eq!(methods, ["PUT", "POST"]);
    Ok(())
}

#[tokio::test]
async fn put_secret_surfaces_any_other_failure() {
    let api = FakeApiServer::new(vec![FakeReply::failure(500, "InternalError")]);

    let result = api.ops().put_secret("work", secret("s")).await;

    assert!(matches!(result, Err(K8sOpsError::Kube(_))));
    assert_eq!(api.seen().len(), 1);
}

#[tokio::test]
async fn read_secret_key_returns_the_stored_bytes() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::object(&secret_with("s", "k", b"value"))?]);

    let value = api.ops().read_secret_key("work", "s", "k").await?;

    assert_eq!(value, Some(b"value".to_vec()));
    assert_eq!(api.seen()[0].method, "GET");
    Ok(())
}

#[tokio::test]
async fn read_secret_key_is_none_for_a_missing_key_or_secret_or_data() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::object(&secret_with("s", "other", b"value"))?,
        FakeReply::failure(404, "NotFound"),
        FakeReply::object(&secret("empty"))?,
    ]);
    let ops = api.ops();

    assert_eq!(ops.read_secret_key("work", "s", "k").await?, None);
    assert_eq!(ops.read_secret_key("work", "gone", "k").await?, None);
    assert_eq!(ops.read_secret_key("work", "empty", "k").await?, None);
    Ok(())
}

#[tokio::test]
async fn read_secret_key_surfaces_a_failed_read() {
    let api = FakeApiServer::new(vec![FakeReply::failure(500, "InternalError")]);

    let result = api.ops().read_secret_key("work", "s", "k").await;

    assert!(matches!(result, Err(K8sOpsError::Kube(_))));
}

#[tokio::test]
async fn deletes_treat_a_missing_object_as_deleted() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::deleted(),
        FakeReply::failure(404, "NotFound"),
        FakeReply::deleted(),
        FakeReply::failure(404, "NotFound"),
        FakeReply::deleted(),
        FakeReply::failure(404, "NotFound"),
        FakeReply::deleted(),
        FakeReply::failure(404, "NotFound"),
    ]);
    let ops = api.ops();

    for _ in 0..2 {
        ops.delete_job("work", "j").await?;
    }
    for _ in 0..2 {
        ops.delete_pod("work", "p").await?;
    }
    for _ in 0..2 {
        ops.delete_secret("work", "s").await?;
    }
    for _ in 0..2 {
        ops.delete_network_policy("work", "np").await?;
    }

    let seen = api.seen();
    assert!(seen.iter().all(|request| request.method == "DELETE"));
    assert!(seen[0].body.contains("Background"), "{}", seen[0].body);
    assert!(seen[2].body.contains("\"gracePeriodSeconds\":0"), "{}", seen[2].body);
    Ok(())
}

#[tokio::test]
async fn deletes_surface_any_other_failure() {
    let api = FakeApiServer::new(Vec::new());
    let ops = api.ops();

    assert!(matches!(ops.delete_job("work", "j").await, Err(K8sOpsError::Kube(_))));
    assert!(matches!(ops.delete_pod("work", "p").await, Err(K8sOpsError::Kube(_))));
    assert!(matches!(
        ops.delete_secret("work", "s").await,
        Err(K8sOpsError::Kube(_))
    ));
    assert!(matches!(
        ops.delete_network_policy("work", "np").await,
        Err(K8sOpsError::Kube(_))
    ));
}

#[tokio::test]
async fn set_secret_owner_patches_the_owner_references() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::object(&secret("s"))?]);
    let owner = OwnerReference {
        api_version: "batch/v1".to_string(),
        kind: "Job".to_string(),
        name: "rs-job".to_string(),
        uid: "uid-1".to_string(),
        ..Default::default()
    };

    api.ops().set_secret_owner("work", "s", owner).await?;

    let seen = api.seen();
    assert_eq!(seen[0].method, "PATCH");
    assert!(seen[0].body.contains("ownerReferences"), "{}", seen[0].body);
    assert!(seen[0].body.contains("uid-1"), "{}", seen[0].body);
    Ok(())
}

#[tokio::test]
async fn find_pod_for_job_returns_the_first_pod_of_the_job() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::list(
        "v1",
        "PodList",
        &[pod("rs-job-a"), pod("rs-job-b")],
    )?]);

    let name = api.ops().find_pod_for_job("work", "rs-job").await?;

    assert_eq!(name, "rs-job-a");
    assert!(
        api.seen()[0].uri.contains("labelSelector=job-name%3Drs-job"),
        "{}",
        api.seen()[0].uri
    );
    Ok(())
}

#[tokio::test]
async fn find_pod_for_job_fails_when_the_job_has_no_pod() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::list(
        "v1",
        "PodList",
        &Vec::<k8s_openapi::api::core::v1::Pod>::new(),
    )?]);

    let result = api.ops().find_pod_for_job("work", "rs-job").await;

    assert!(
        matches!(&result, Err(K8sOpsError::NoPodForJob { job_name }) if job_name == "rs-job"),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn find_pod_for_job_fails_when_the_listing_fails() {
    let api = FakeApiServer::new(Vec::new());

    let result = api.ops().find_pod_for_job("work", "rs-job").await;

    assert!(matches!(result, Err(K8sOpsError::Kube(_))));
}

fn labels() -> BTreeMap<String, String> {
    [("lease".to_string(), "l-1".to_string())].into()
}

#[tokio::test]
async fn patch_pod_labels_fences_on_the_resource_version() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::object(&pod("p"))?]);

    let outcome = api.ops().patch_pod_labels("work", "p", labels(), "77").await?;

    assert_eq!(outcome, FencedPatchOutcome::Applied);
    let seen = api.seen();
    assert_eq!(seen[0].method, "PATCH");
    assert!(seen[0].body.contains("\"resourceVersion\":\"77\""), "{}", seen[0].body);
    assert!(seen[0].body.contains("l-1"), "{}", seen[0].body);
    Ok(())
}

#[tokio::test]
async fn a_stale_resource_version_is_a_failed_precondition_not_an_error() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::failure(409, "Conflict")]);

    let outcome = api.ops().patch_pod_labels("work", "p", labels(), "77").await?;

    assert_eq!(outcome, FencedPatchOutcome::PreconditionFailed);
    Ok(())
}

#[tokio::test]
async fn an_already_exists_conflict_is_still_an_error() {
    let api = FakeApiServer::new(vec![FakeReply::failure(409, "AlreadyExists")]);

    let result = api.ops().patch_pod_labels("work", "p", labels(), "77").await;

    assert!(matches!(result, Err(K8sOpsError::Kube(_))));
}

#[tokio::test]
async fn patch_config_map_merges_the_data() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::object(&config_map("cm"))?,
        FakeReply::failure(500, "InternalError"),
    ]);
    let ops = api.ops();
    let data: BTreeMap<String, String> = [("key".to_string(), "value".to_string())].into();

    ops.patch_config_map("work", "cm", data.clone()).await?;
    let failed = ops.patch_config_map("work", "cm", data).await;

    let seen = api.seen();
    assert_eq!(seen[0].method, "PATCH");
    assert!(seen[0].body.contains("\"key\":\"value\""), "{}", seen[0].body);
    assert!(matches!(failed, Err(K8sOpsError::Kube(_))));
    Ok(())
}

#[tokio::test]
async fn patch_network_policy_updates_an_existing_policy() -> TestResult {
    let api = FakeApiServer::new(vec![FakeReply::object(&network_policy("np"))?]);

    api.ops()
        .patch_network_policy("work", "np", NetworkPolicySpec::default())
        .await?;

    let methods: Vec<_> = api.seen().into_iter().map(|request| request.method).collect();
    assert_eq!(methods, ["PATCH"]);
    Ok(())
}

#[tokio::test]
async fn patch_network_policy_creates_the_policy_a_lease_never_had() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::failure(404, "NotFound"),
        FakeReply::object(&network_policy("np"))?,
    ]);

    api.ops()
        .patch_network_policy("work", "np", NetworkPolicySpec::default())
        .await?;

    let seen = api.seen();
    assert_eq!(seen[1].method, "POST");
    assert!(seen[1].body.contains("\"name\":\"np\""), "{}", seen[1].body);
    assert!(seen[1].body.contains("\"namespace\":\"work\""), "{}", seen[1].body);
    Ok(())
}

#[tokio::test]
async fn patch_network_policy_surfaces_any_other_failure() {
    let api = FakeApiServer::new(Vec::new());

    let result = api
        .ops()
        .patch_network_policy("work", "np", NetworkPolicySpec::default())
        .await;

    assert!(matches!(result, Err(K8sOpsError::Kube(_))));
}
