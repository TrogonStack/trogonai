use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::jiff::Timestamp;

use super::fake_api_server_tests::FakeApiServer;
use super::fake_reply_tests::FakeReply;
use super::test_objects_tests::{job, pod};
use crate::K8sOpsError;
use crate::k8s_ops::{K8sOps, RealK8sOps};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DATE: &str = "Tue, 15 Nov 1994 08:12:31 GMT";

fn pod_list(pods: &[Pod]) -> Result<FakeReply, serde_json::Error> {
    FakeReply::list("v1", "PodList", pods)
}

#[tokio::test]
async fn list_pods_returns_the_pods_with_the_apiserver_clock() -> TestResult {
    let api = FakeApiServer::new(vec![pod_list(&[pod("a"), pod("b")])?.with_date(DATE)]);

    let listing = api.ops().list_pods("work", "app=sandbox,tier in (a b)").await?;

    assert_eq!(listing.pods.len(), 2);
    assert_eq!(listing.clock, Some("1994-11-15T08:12:31Z".parse::<Timestamp>()?));
    let seen = api.seen();
    assert_eq!(seen[0].method, "GET");
    assert!(
        seen[0]
            .uri
            .starts_with("/api/v1/namespaces/work/pods?labelSelector=app%3Dsandbox%2Ctier+in+%28a+b%29"),
        "{}",
        seen[0].uri
    );
    Ok(())
}

#[tokio::test]
async fn list_jobs_returns_the_jobs_with_the_apiserver_clock() -> TestResult {
    let api = FakeApiServer::new(vec![
        FakeReply::list("batch/v1", "JobList", &[job("j")])?.with_date(DATE),
    ]);

    let listing = api.ops().list_jobs("work", "app=sandbox").await?;

    assert_eq!(listing.jobs.len(), 1);
    assert!(listing.clock.is_some());
    assert!(
        api.seen()[0].uri.starts_with("/apis/batch/v1/namespaces/work/jobs?"),
        "{}",
        api.seen()[0].uri
    );
    Ok(())
}

#[tokio::test]
async fn a_listing_without_a_usable_date_has_no_clock() -> TestResult {
    let api = FakeApiServer::new(vec![
        pod_list(&[])?,
        pod_list(&[])?.with_date("not a date"),
        FakeReply::list("batch/v1", "JobList", &Vec::<Job>::new())?,
    ]);
    let ops = api.ops();

    assert_eq!(ops.list_pods("work", "").await?.clock, None);
    assert_eq!(ops.list_pods("work", "").await?.clock, None);
    assert_eq!(ops.list_jobs("work", "").await?.clock, None);
    Ok(())
}

#[tokio::test]
async fn a_failed_listing_reports_its_status_and_body() {
    let api = FakeApiServer::new(vec![FakeReply::failure(403, "Forbidden")]);

    let result = api.ops().list_pods("work", "").await;

    assert!(
        matches!(&result, Err(K8sOpsError::ListStatus { path, status, body })
            if path == "/api/v1/namespaces/work/pods" && status.as_u16() == 403 && body.contains("Forbidden")),
        "{:?}",
        result.err()
    );
}

#[tokio::test]
async fn an_undecodable_listing_is_a_decode_error() {
    let api = FakeApiServer::new(vec![FakeReply::text(200, "not json")]);

    let result = api.ops().list_jobs("work", "").await;

    assert!(matches!(result, Err(K8sOpsError::ListDecode(_))));
}

#[tokio::test]
async fn a_listing_body_that_breaks_midway_is_a_body_error() {
    let api = FakeApiServer::new(vec![FakeReply::broken_body()]);

    let result = api.ops().list_pods("work", "").await;

    assert!(matches!(result, Err(K8sOpsError::ListBody(_))));
}

#[tokio::test]
async fn an_unreachable_apiserver_is_a_send_error() {
    let ops = RealK8sOps::new(FakeApiServer::unreachable_client());

    let result = ops.list_pods("work", "").await;

    assert!(
        matches!(&result, Err(K8sOpsError::ListSend { path, .. }) if path == "/api/v1/namespaces/work/pods"),
        "{:?}",
        result.err()
    );
}

#[tokio::test]
async fn a_namespace_that_is_not_a_valid_uri_is_a_request_error() {
    let api = FakeApiServer::new(Vec::new());

    let result = api.ops().list_pods("not a namespace", "").await;

    assert!(matches!(result, Err(K8sOpsError::ListRequest(_))));
    assert!(api.seen().is_empty());
}
