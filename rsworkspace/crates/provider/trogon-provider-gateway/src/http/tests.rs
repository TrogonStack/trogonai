use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response},
};
use tower::ServiceExt;

async fn send(app: Router, method: Method, uri: &str) -> Response<Body> {
    app.oneshot(Request::builder().method(method).uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn assert_empty(response: Response<Body>) {
    assert!(to_bytes(response.into_body(), usize::MAX).await.unwrap().is_empty());
}

#[tokio::test]
async fn canonical_operational_probes_report_only_status() {
    let app = router(Health::default());
    for path in ["/-/liveness", "/-/readiness"] {
        for method in [Method::GET, Method::HEAD] {
            let response = send(app.clone(), method, path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_empty(response).await;
        }
    }
}

#[tokio::test]
async fn draining_fails_readiness_while_liveness_keeps_passing() {
    let health = Health::default();
    let app = router(health.clone());
    health.begin_shutdown();
    health.begin_shutdown();

    for (path, expected) in [
        ("/-/readiness", StatusCode::SERVICE_UNAVAILABLE),
        ("/-/liveness", StatusCode::OK),
    ] {
        for method in [Method::GET, Method::HEAD] {
            let response = send(app.clone(), method, path).await;
            assert_eq!(response.status(), expected, "{path}");
            assert_empty(response).await;
        }
    }
}

#[tokio::test]
async fn wrong_method_is_rejected() {
    let app = router(Health::default());
    for path in ["/-/liveness", "/-/readiness"] {
        let response = send(app.clone(), Method::POST, path).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}

#[tokio::test]
async fn application_and_legacy_probe_routes_are_absent() {
    let app = router(Health::default());
    for path in [
        "/",
        "/healthz",
        "/v1/models",
        "/v1/messages",
        "/management.html",
        "/-/metrics",
    ] {
        let response = send(app.clone(), Method::GET, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
