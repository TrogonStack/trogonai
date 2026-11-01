use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode as AxumStatus;
use axum::routing::post;
use serde_json::Value;

use super::*;
use crate::source::plaid::test_support::TestKeyPair;

#[derive(Clone)]
struct Stub {
    status: AxumStatus,
    body: Value,
    requests: Arc<Mutex<Vec<Value>>>,
}

async fn handle(State(stub): State<Stub>, Json(request): Json<Value>) -> (AxumStatus, Json<Value>) {
    stub.requests.lock().unwrap().push(request);
    (stub.status, Json(stub.body.clone()))
}

async fn serve(status: AxumStatus, body: Value) -> (PlaidApiBaseUrl, Arc<Mutex<Vec<Value>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let stub = Stub {
        status,
        body,
        requests: Arc::clone(&requests),
    };
    let app = Router::new()
        .route("/webhook_verification_key/get", post(handle))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (PlaidApiBaseUrl::new(format!("http://{addr}")).unwrap(), requests)
}

async fn serve_raw(status: AxumStatus, body: &'static str) -> (PlaidApiBaseUrl, Arc<Mutex<Vec<Value>>>) {
    let app = Router::new().route(
        "/webhook_verification_key/get",
        post(move || async move { (status, body) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        PlaidApiBaseUrl::new(format!("http://{addr}")).unwrap(),
        Arc::new(Mutex::new(Vec::new())),
    )
}

fn fetcher(url: &PlaidApiBaseUrl) -> ReqwestPlaidKeyFetcher {
    ReqwestPlaidKeyFetcher::new(url, PlaidClientId::new("client-1").unwrap())
}

fn secret() -> PlaidSecret {
    PlaidSecret::new("secret-1").unwrap()
}

#[tokio::test]
async fn posts_credentials_and_key_id_and_parses_the_key() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, requests) = serve(AxumStatus::OK, pair.jwk_json(None)).await;

    let key = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap();

    assert_eq!(key.key_id().as_str(), "kid-1");
    assert!(!key.is_expired_at(std::time::SystemTime::now()));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["client_id"], "client-1");
    assert_eq!(requests[0]["secret"], "secret-1");
    assert_eq!(requests[0]["key_id"], "kid-1");
}

#[tokio::test]
async fn posts_the_secret_given_to_each_call() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, requests) = serve(AxumStatus::OK, pair.jwk_json(None)).await;
    let fetcher = fetcher(&url);

    fetcher
        .fetch(&pair.key_id(), &PlaidSecret::new("secret-first").unwrap())
        .await
        .unwrap();
    fetcher
        .fetch(&pair.key_id(), &PlaidSecret::new("secret-second").unwrap())
        .await
        .unwrap();

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["secret"], "secret-first");
    assert_eq!(requests[1]["secret"], "secret-second");
}

#[tokio::test]
async fn carries_the_expiry_of_the_returned_key() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve(AxumStatus::OK, pair.jwk_json(Some(1_000))).await;

    let key = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap();

    assert!(key.is_expired_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000)));
}

#[tokio::test]
async fn server_errors_are_unavailable() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve(AxumStatus::BAD_GATEWAY, serde_json::json!({})).await;

    let err = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap_err();

    assert!(matches!(&err, PlaidKeyFetchError::Unavailable(status) if *status == StatusCode::BAD_GATEWAY));
}

#[tokio::test]
async fn rate_limiting_is_unavailable() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve(AxumStatus::TOO_MANY_REQUESTS, serde_json::json!({})).await;

    let err = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap_err();

    assert!(matches!(err, PlaidKeyFetchError::Unavailable(_)));
}

#[tokio::test]
async fn client_errors_are_rejected() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve(
        AxumStatus::BAD_REQUEST,
        serde_json::json!({"error_code": "INVALID_API_KEYS"}),
    )
    .await;

    let err = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap_err();

    assert!(matches!(&err, PlaidKeyFetchError::Rejected(status) if *status == StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn unknown_verification_key_id_is_unknown_key() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve(
        AxumStatus::BAD_REQUEST,
        serde_json::json!({"error_type": "INVALID_INPUT", "error_code": "INVALID_WEBHOOK_VERIFICATION_KEY_ID"}),
    )
    .await;

    let err = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap_err();

    assert!(matches!(err, PlaidKeyFetchError::UnknownKey));
}

#[tokio::test]
async fn client_error_with_a_non_json_body_is_rejected() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve_raw(AxumStatus::BAD_REQUEST, "not json").await;

    let err = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap_err();

    assert!(matches!(&err, PlaidKeyFetchError::Rejected(status) if *status == StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn unparseable_success_body_is_a_transport_failure() {
    let pair = TestKeyPair::generate("kid-1");
    let (url, _) = serve(AxumStatus::OK, serde_json::json!({"unexpected": true})).await;

    let err = fetcher(&url).fetch(&pair.key_id(), &secret()).await.unwrap_err();

    assert!(matches!(err, PlaidKeyFetchError::Transport(_)));
}

#[tokio::test]
async fn a_key_for_a_different_kid_is_invalid() {
    let served = TestKeyPair::generate("kid-other");
    let (url, _) = serve(AxumStatus::OK, served.jwk_json(None)).await;

    let err = fetcher(&url)
        .fetch(&PlaidKeyId::new("kid-1").unwrap(), &secret())
        .await
        .unwrap_err();

    assert!(matches!(
        &err,
        PlaidKeyFetchError::InvalidKey(PlaidVerificationKeyError::KeyIdMismatch { .. })
    ));
}

#[tokio::test]
async fn connection_failure_is_a_transport_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let url = PlaidApiBaseUrl::new(format!("http://{addr}")).unwrap();

    let err = fetcher(&url)
        .fetch(&PlaidKeyId::new("kid-1").unwrap(), &secret())
        .await
        .unwrap_err();

    assert!(matches!(err, PlaidKeyFetchError::Transport(_)));
}
