use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, header},
};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn send(method: Method, uri: &str) -> Response<Body> {
    router()
        .oneshot(Request::builder().method(method).uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_bytes(response: Response<Body>) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
}

fn content_type(response: &Response<Body>) -> &str {
    response.headers()[header::CONTENT_TYPE].to_str().unwrap()
}

#[tokio::test]
async fn root_describes_the_gateway() {
    let response = send(Method::GET, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), "application/json");
    let body: Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(
        body,
        json!({
            "message": "Trogon Provider Gateway",
            "endpoints": ["GET /healthz", "HEAD /healthz"],
        })
    );
}

#[tokio::test]
async fn healthz_reports_ok() {
    let response = send(Method::GET, "/healthz").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), "application/json");
    let body: Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn healthz_head_has_empty_body() {
    let response = send(Method::HEAD, "/healthz").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_bytes(response).await.is_empty());
}

#[tokio::test]
async fn wrong_method_is_rejected() {
    for (method, uri) in [
        (Method::POST, "/"),
        (Method::POST, "/healthz"),
        (Method::DELETE, "/healthz"),
    ] {
        let response = send(method, uri).await;

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}

#[tokio::test]
async fn provider_routes_are_absent() {
    for (method, uri) in [
        (Method::GET, "/v1/models"),
        (Method::POST, "/v1/chat/completions"),
        (Method::POST, "/v1/messages"),
        (Method::GET, "/management.html"),
        (Method::GET, "/-/liveness"),
        (Method::GET, "/-/readiness"),
    ] {
        let response = send(method, uri).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
