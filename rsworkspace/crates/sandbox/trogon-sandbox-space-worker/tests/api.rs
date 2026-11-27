mod support;

use std::time::Duration;

use support::{TestResult, api};
use trogon_sandbox_space_worker::{ApiError, SpaceId};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn id() -> Result<SpaceId, Box<dyn std::error::Error>> {
    Ok(SpaceId::parse("sb-1")?)
}

async fn serving(
    response: ResponseTemplate,
) -> Result<(MockServer, trogon_sandbox_space_worker::SpaceApi), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(any()).respond_with(response).mount(&server).await;
    let client = api(&server.uri(), &server.uri())?;
    Ok((server, client))
}

#[tokio::test]
async fn an_empty_success_body_is_not_a_process_list() -> TestResult {
    let (_server, client) = serving(ResponseTemplate::new(200)).await?;
    let error = client
        .probe_processes(&id()?, Duration::from_secs(1))
        .await
        .err()
        .ok_or("probed")?;
    assert!(matches!(error, ApiError::Protocol(_)));
    assert!(!format!("{error}").is_empty());
    assert!(!error.is_retryable());
    assert!(!error.definitely_not_applied());
    assert!(error.code().is_none());
    assert!(error.reason().is_none());
    Ok(())
}

#[tokio::test]
async fn a_server_error_other_than_a_transient_one_is_final() -> TestResult {
    let (_server, client) = serving(ResponseTemplate::new(500)).await?;
    let error = client.sandbox(&id()?).await.err().ok_or("found")?;
    assert!(matches!(
        error,
        ApiError::Status {
            status: 500,
            retryable: false,
            ..
        }
    ));
    assert_eq!(error.code(), Some("http_500"));
    assert!(!format!("{error}").is_empty());
    Ok(())
}

#[tokio::test]
async fn reads_the_envelope_at_the_top_level() -> TestResult {
    let body = serde_json::json!({
        "code": "sandbox_not_running", "message": "stopped", "request_id": "req-9",
        "details": [{"other": 1}, {"reason": "sandbox_not_running"}],
    });
    let (_server, client) = serving(ResponseTemplate::new(409).set_body_json(body)).await?;
    let error = client
        .probe_processes(&id()?, Duration::from_secs(1))
        .await
        .err()
        .ok_or("probed")?;
    assert_eq!(error.code(), Some("sandbox_not_running"));
    assert_eq!(error.reason(), Some("sandbox_not_running"));
    assert!(!format!("{error}").is_empty());
    assert!(!error.is_not_found());
    Ok(())
}

#[tokio::test]
async fn a_status_without_details_has_no_reason() -> TestResult {
    let (_server, client) = serving(ResponseTemplate::new(409)).await?;
    let error = client
        .probe_processes(&id()?, Duration::from_secs(1))
        .await
        .err()
        .ok_or("probed")?;
    assert!(error.reason().is_none());
    Ok(())
}

#[tokio::test]
async fn a_reply_that_never_finishes_is_ambiguous() -> TestResult {
    let (_server, client) = serving(ResponseTemplate::new(200).set_delay(Duration::from_secs(2))).await?;
    let error = client
        .probe_processes(&id()?, Duration::from_millis(50))
        .await
        .err()
        .ok_or("probed")?;
    assert!(matches!(error, ApiError::Ambiguous(_)));
    assert!(error.is_retryable());
    assert!(!error.definitely_not_applied());
    assert!(!format!("{error}").is_empty());
    Ok(())
}

#[tokio::test]
async fn refuses_a_reply_body_that_is_too_large() -> TestResult {
    let body = vec![b'x'; 8 * 1024 * 1024 + 1];
    let (_server, client) = serving(ResponseTemplate::new(200).set_body_bytes(body)).await?;
    let error = client
        .probe_processes(&id()?, Duration::from_secs(5))
        .await
        .err()
        .ok_or("probed")?;
    assert!(!format!("{error}").is_empty());
    Ok(())
}

#[tokio::test]
async fn an_unreachable_server_did_not_apply_the_request() -> TestResult {
    let closed = support::closed_origin().await?;
    let client = api(&closed, &closed)?;
    let error = client
        .probe_processes(&id()?, Duration::from_secs(1))
        .await
        .err()
        .ok_or("probed")?;
    assert!(matches!(error, ApiError::NotApplied(_)));
    assert!(error.is_retryable());
    assert!(error.definitely_not_applied());
    assert!(!format!("{error}").is_empty());
    Ok(())
}
