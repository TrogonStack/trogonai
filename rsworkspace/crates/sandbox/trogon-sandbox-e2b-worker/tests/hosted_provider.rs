//! The hosted provider's lease lifecycle against an in-process stand-in for the E2B control plane: the native sandbox
//! exists while the lease is held, is gone once the lease is released, and a delete that fails keeps the lease held.
//! Nothing here reaches the real service.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use trogon_sandbox_contract::{LeaseId, LeaseRequest};
use trogon_sandbox_e2b_worker::{ApiError, E2bApi, E2bConfig, E2bExec, E2bWorkerProvider, Envd};
use trogon_sandbox_protocol::Provider;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Hosted {
    _server: MockServer,
    api: E2bApi,
    provider: Arc<E2bWorkerProvider>,
    deletes: Arc<AtomicUsize>,
}

fn request() -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json!({
        "tenant": "hosted-probe",
        "provider_pref": ["e2b"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "base"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 180,
        "idempotency_key": "hosted-probe",
    }))?)
}

/// A control plane whose first `failing_deletes` deletes answer 503, after which a delete removes the sandbox.
async fn hosted(failing_deletes: usize) -> Result<Hosted, Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let gone = Arc::new(AtomicBool::new(false));
    let deletes = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/v2/sandboxes"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"sandboxID": "sbx-a", "templateID": "base"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sandboxes/sbx-a"))
        .respond_with({
            let gone = gone.clone();
            move |_: &Request| {
                ResponseTemplate::new(if gone.load(Ordering::SeqCst) { 404 } else { 200 })
                    .set_body_json(json!({"state": "running"}))
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/sandboxes/sbx-a"))
        .respond_with({
            let deletes = deletes.clone();
            move |_: &Request| {
                let failing = deletes.fetch_add(1, Ordering::SeqCst) < failing_deletes;
                gone.store(!failing, Ordering::SeqCst);
                ResponseTemplate::new(if failing { 503 } else { 204 })
            }
        })
        .mount(&server)
        .await;
    let origin = Url::parse(&server.uri())?;
    let config = E2bConfig::new(origin.clone(), "e2b.test", "k")?.with_envd_url(origin)?;
    let api = E2bApi::new(config, Duration::from_secs(5))?;
    let provider = Arc::new(E2bWorkerProvider::new(
        api.clone(),
        "base",
        Arc::new(E2bExec::new(Envd::new(Duration::from_secs(3))?)),
    ));
    Ok(Hosted {
        _server: server,
        api,
        provider,
        deletes,
    })
}

#[tokio::test]
async fn the_native_sandbox_is_present_while_leased_and_absent_once_released() -> TestResult {
    let hosted = hosted(0).await?;
    let id = LeaseId::new("hosted-probe");
    let granted = hosted.provider.lease(&request()?, &id).await?;
    assert_eq!(granted.sandbox_id.as_str(), "sbx-a");
    assert_eq!(hosted.api.get_sandbox("sbx-a").await?.state, "running");

    hosted.provider.cancel(&id).await?;
    assert_eq!(hosted.provider.active_lease_count().await, 0);
    assert_eq!(hosted.deletes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        hosted.api.get_sandbox("sbx-a").await,
        Err(ApiError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_cannot_be_deleted_stays_leased_until_the_delete_succeeds() -> TestResult {
    let hosted = hosted(1).await?;
    let id = LeaseId::new("hosted-probe");
    hosted.provider.lease(&request()?, &id).await?;
    assert!(hosted.provider.cancel(&id).await.is_err());
    assert_eq!(hosted.provider.active_lease_count().await, 1);
    hosted.provider.cancel(&id).await?;
    assert_eq!(hosted.provider.active_lease_count().await, 0);
    Ok(())
}
