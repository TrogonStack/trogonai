use super::*;
use axum::Router;
use axum::http::HeaderName;
use axum::http::{Method, Request, StatusCode};
use axum::routing::{get, post};
use std::net::Ipv4Addr;
use tower::ServiceExt;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const PUBLIC: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));

fn origin_app(bind_host: IpAddr) -> Router {
    Router::new()
        .route(
            "/acp",
            get(|| async { "ok" }).post(|| async { "ok" }).delete(|| async { "ok" }),
        )
        .layer(axum::middleware::from_fn(move |request, next| {
            enforce_origin(bind_host, request, next)
        }))
}

fn request_with(method: Method, headers: &[(&str, &str)]) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri("/acp");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

async fn origin_status(bind_host: IpAddr, method: Method, headers: &[(&str, &str)]) -> StatusCode {
    origin_app(bind_host)
        .oneshot(request_with(method, headers))
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_request_without_an_origin_is_allowed() {
    assert_eq!(origin_status(LOOPBACK, Method::POST, &[]).await, StatusCode::OK);
}

#[tokio::test]
async fn loopback_origins_are_allowed_when_bound_to_loopback() {
    for origin in ["http://localhost:8080", "http://127.0.0.1:8080", "http://[::1]:8080"] {
        assert_eq!(
            origin_status(LOOPBACK, Method::POST, &[("origin", origin)]).await,
            StatusCode::OK,
            "{origin} should be allowed"
        );
    }
}

#[tokio::test]
async fn a_remote_origin_is_rejected_when_bound_to_loopback() {
    assert_eq!(
        origin_status(LOOPBACK, Method::POST, &[("origin", "http://evil.example")]).await,
        StatusCode::FORBIDDEN
    );
}

/// The gap this layer exists to close: upstream checks `Origin` server-side only
/// on the WebSocket upgrade, so every other verb must be covered here.
#[tokio::test]
async fn every_verb_enforces_origin_not_just_the_upgrade() {
    for method in [Method::GET, Method::POST, Method::DELETE] {
        assert_eq!(
            origin_status(LOOPBACK, method.clone(), &[("origin", "http://evil.example")]).await,
            StatusCode::FORBIDDEN,
            "{method} must enforce Origin"
        );
    }
}

#[tokio::test]
async fn a_malformed_origin_is_rejected() {
    assert_eq!(
        origin_status(LOOPBACK, Method::POST, &[("origin", "not a uri")]).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn an_origin_matching_the_request_host_is_allowed_when_bound_publicly() {
    assert_eq!(
        origin_status(
            PUBLIC,
            Method::POST,
            &[("origin", "http://acp.example"), ("host", "acp.example")]
        )
        .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_origin_matching_the_bind_host_is_allowed() {
    assert_eq!(
        origin_status(PUBLIC, Method::POST, &[("origin", "http://203.0.113.7")]).await,
        StatusCode::OK
    );
}

fn versions_app(versions: NegotiatedVersions, body: &'static str, content_type: &'static str) -> Router {
    Router::new()
        .route(
            "/acp",
            post(move || async move {
                (
                    [
                        (CONTENT_TYPE, content_type),
                        (HeaderName::from_static(ACP_CONNECTION_ID_HEADER), "conn-1"),
                    ],
                    body,
                )
            })
            .get(|| async { ([(CONTENT_TYPE, "text/event-stream")], "data: {}\n\n") })
            .delete(|| async { "gone" }),
        )
        .layer(axum::middleware::from_fn(move |request, next| {
            track_protocol_version(versions.clone(), request, next)
        }))
}

const INITIALIZE_REPLY: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#;

#[tokio::test]
async fn the_negotiated_version_is_learned_from_the_initialize_reply() {
    let versions = NegotiatedVersions::new();
    let app = versions_app(versions.clone(), INITIALIZE_REPLY, "application/json");

    let response = app.oneshot(request_with(Method::POST, &[])).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // The body must survive being inspected.
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.as_ref(), INITIALIZE_REPLY.as_bytes());

    assert_eq!(versions.get("conn-1"), Some(1));
}

#[tokio::test]
async fn a_matching_protocol_version_header_is_allowed() {
    let versions = NegotiatedVersions::new();
    versions.record("conn-1".to_owned(), 1);

    let status = versions_app(versions, INITIALIZE_REPLY, "application/json")
        .oneshot(request_with(
            Method::POST,
            &[(ACP_CONNECTION_ID_HEADER, "conn-1"), (ACP_PROTOCOL_VERSION_HEADER, "1")],
        ))
        .await
        .unwrap()
        .status();

    assert_eq!(status, StatusCode::OK);
}

/// The behavior upstream has no equivalent for: the transport spec says clients
/// SHOULD send this header, and a value disagreeing with what the connection
/// negotiated is a client bug rather than something to serve anyway.
#[tokio::test]
async fn a_mismatched_protocol_version_header_is_rejected() {
    let versions = NegotiatedVersions::new();
    versions.record("conn-1".to_owned(), 1);

    let status = versions_app(versions, INITIALIZE_REPLY, "application/json")
        .oneshot(request_with(
            Method::POST,
            &[(ACP_CONNECTION_ID_HEADER, "conn-1"), (ACP_PROTOCOL_VERSION_HEADER, "0")],
        ))
        .await
        .unwrap()
        .status();

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Upstream owns connection lifetime, so an id this layer has never seen
/// initialize is passed through rather than rejected on a guess.
#[tokio::test]
async fn a_header_for_an_unknown_connection_is_allowed() {
    let status = versions_app(NegotiatedVersions::new(), INITIALIZE_REPLY, "application/json")
        .oneshot(request_with(
            Method::POST,
            &[
                (ACP_CONNECTION_ID_HEADER, "never-seen"),
                (ACP_PROTOCOL_VERSION_HEADER, "9"),
            ],
        ))
        .await
        .unwrap()
        .status();

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn delete_forgets_the_connection() {
    let versions = NegotiatedVersions::new();
    versions.record("conn-1".to_owned(), 1);

    let _ = versions_app(versions.clone(), INITIALIZE_REPLY, "application/json")
        .oneshot(request_with(Method::DELETE, &[(ACP_CONNECTION_ID_HEADER, "conn-1")]))
        .await
        .unwrap();

    assert_eq!(
        versions.get("conn-1"),
        None,
        "a terminated connection must not be retained"
    );
}

/// A streaming response must never be buffered to read a version out of it.
#[tokio::test]
async fn an_event_stream_response_is_not_inspected() {
    let versions = NegotiatedVersions::new();
    let response = versions_app(versions.clone(), INITIALIZE_REPLY, "application/json")
        .oneshot(request_with(Method::GET, &[]))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(versions.get("conn-1"), None);
}

#[tokio::test]
async fn a_non_json_response_is_not_inspected() {
    let versions = NegotiatedVersions::new();
    let _ = versions_app(versions.clone(), INITIALIZE_REPLY, "text/plain")
        .oneshot(request_with(Method::POST, &[]))
        .await
        .unwrap();

    assert_eq!(versions.get("conn-1"), None);
}

fn buffering_app(content_type: &'static str) -> Router {
    Router::new()
        .route(
            "/acp",
            get(move || async move { ([(CONTENT_TYPE, content_type)], "body") }),
        )
        .layer(axum::middleware::from_fn(disable_sse_buffering))
}

#[tokio::test]
async fn the_sse_stream_disables_proxy_buffering() {
    let response = buffering_app("text/event-stream")
        .oneshot(request_with(Method::GET, &[]))
        .await
        .unwrap();

    assert_eq!(
        response.headers().get(X_ACCEL_BUFFERING_HEADER).unwrap(),
        "no",
        "a long-lived stream must not sit in a proxy buffer"
    );
}

#[tokio::test]
async fn a_json_response_is_left_alone() {
    let response = buffering_app("application/json")
        .oneshot(request_with(Method::GET, &[]))
        .await
        .unwrap();

    assert!(response.headers().get(X_ACCEL_BUFFERING_HEADER).is_none());
}
