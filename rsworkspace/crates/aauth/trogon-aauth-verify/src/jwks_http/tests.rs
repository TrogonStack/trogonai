use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn empty_jwks_body() -> String {
    serde_json::json!({ "keys": [] }).to_string()
}

#[test]
fn well_known_url_joins_base_and_dwk() {
    let dwk = WellKnownDwk::new(DWK_AGENT).unwrap();
    assert_eq!(
        well_known_url("https://issuer.example", &dwk),
        "https://issuer.example/.well-known/aauth-agent.json"
    );
}

#[test]
fn well_known_url_strips_trailing_slash_from_base() {
    let dwk = WellKnownDwk::new(DWK_AGENT).unwrap();
    let base = require_https_issuer("https://issuer.example/").unwrap();
    assert_eq!(
        well_known_url(base, &dwk),
        "https://issuer.example/.well-known/aauth-agent.json"
    );
}

#[test]
fn default_dwk_order_is_agent_resource_person() {
    let order = default_dwk_order();
    assert_eq!(
        order.iter().map(WellKnownDwk::as_str).collect::<Vec<_>>(),
        vec![DWK_AGENT, DWK_RESOURCE, DWK_PERSON]
    );
}

#[test]
fn well_known_dwk_rejects_empty() {
    assert!(matches!(WellKnownDwk::new(""), Err(WellKnownDwkError::Empty)));
    assert!(matches!(WellKnownDwk::new("   "), Err(WellKnownDwkError::Empty)));
}

#[test]
fn well_known_dwk_rejects_embedded_slash() {
    assert!(matches!(
        WellKnownDwk::new("a/b.json"),
        Err(WellKnownDwkError::ContainsSlash(_))
    ));
}

#[test]
fn require_https_issuer_accepts_https() {
    assert_eq!(
        require_https_issuer("https://issuer.example").unwrap(),
        "https://issuer.example"
    );
}

#[test]
fn require_https_issuer_rejects_http() {
    let err = require_https_issuer("http://issuer.example").unwrap_err();
    assert!(matches!(err, JwksError::Transport(_)));
}

#[test]
fn require_https_issuer_rejects_scheme_relative() {
    assert!(require_https_issuer("issuer.example").is_err());
}

#[test]
fn max_response_bytes_rejects_zero() {
    assert!(matches!(MaxResponseBytes::new(0), Err(MaxResponseBytesError::Zero)));
    assert!(MaxResponseBytes::new(1).is_ok());
}

#[test]
fn request_timeout_rejects_zero() {
    assert!(matches!(
        RequestTimeout::new(Duration::ZERO),
        Err(RequestTimeoutError::Zero)
    ));
    assert!(RequestTimeout::new(Duration::from_millis(1)).is_ok());
}

/// `resolve()` itself refuses non-https issuers before any network call, so
/// exercising it end-to-end against a live server would never reach the
/// network path for an `http://` wiremock URI. Assert the guard directly via
/// the public trait method instead.
#[tokio::test]
async fn resolve_rejects_http_issuer_without_network_call() {
    let resolver = HttpJwksResolver::new();
    let err = resolver.resolve("http://issuer.example").await.unwrap_err();
    assert!(matches!(err, JwksError::Transport(_)));
}

#[tokio::test]
async fn resolve_from_base_tries_dwks_in_order_until_one_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/aauth-agent.json"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/aauth-resource.json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(empty_jwks_body(), "application/json"))
        .mount(&server)
        .await;

    let resolver = HttpJwksResolver::new();
    let set = resolver
        .resolve_from_base("https://issuer.example", &server.uri())
        .await
        .expect("second dwk in order should resolve");
    assert!(set.keys.is_empty());
}

#[tokio::test]
async fn resolve_from_base_fails_when_all_dwks_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let resolver = HttpJwksResolver::new();
    let err = resolver
        .resolve_from_base("https://issuer.example", &server.uri())
        .await
        .unwrap_err();
    match err {
        JwksError::Transport(msg) => {
            assert!(msg.contains("aauth-agent.json"));
            assert!(msg.contains("aauth-resource.json"));
            assert!(msg.contains("aauth-person.json"));
        }
        other => panic!("expected Transport error naming attempted dwks, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_from_base_reports_malformed_json_when_no_dwk_parses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("not json", "application/json"))
        .mount(&server)
        .await;

    let resolver = HttpJwksResolver::new();
    let err = resolver
        .resolve_from_base("https://issuer.example", &server.uri())
        .await
        .unwrap_err();
    assert!(matches!(err, JwksError::Transport(msg) if msg.contains("malformed JWKS JSON")));
}

#[tokio::test]
async fn resolve_from_base_respects_custom_dwk_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/aauth-person.json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(empty_jwks_body(), "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/aauth-agent.json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(empty_jwks_body(), "application/json"))
        .mount(&server)
        .await;

    // Person is listed first, so it must win even though agent would also
    // succeed -- proves ordering, not just "first working dwk in default
    // order", drives the outcome.
    let resolver = HttpJwksResolver::with_config(
        vec![
            WellKnownDwk::new(DWK_PERSON).unwrap(),
            WellKnownDwk::new(DWK_AGENT).unwrap(),
        ],
        RequestTimeout::default(),
        MaxResponseBytes::default(),
    );
    let calls_person = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _ = &calls_person; // wiremock request log inspected via server.received_requests below.

    resolver
        .resolve_from_base("https://issuer.example", &server.uri())
        .await
        .expect("person dwk should resolve first");

    let requests = server.received_requests().await.expect("request log");
    assert_eq!(requests.len(), 1, "should stop after the first successful dwk");
    assert_eq!(requests[0].url.path(), "/.well-known/aauth-person.json");
}

#[tokio::test]
async fn resolve_from_base_enforces_response_size_cap_via_content_length() {
    let server = MockServer::start().await;
    let big_body = format!(r#"{{"keys": [], "padding": "{}"}}"#, "a".repeat(64));
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(big_body, "application/json"))
        .mount(&server)
        .await;

    let resolver = HttpJwksResolver::with_config(
        default_dwk_order(),
        RequestTimeout::default(),
        MaxResponseBytes::new(8).unwrap(),
    );
    let err = resolver
        .resolve_from_base("https://issuer.example", &server.uri())
        .await
        .unwrap_err();
    match err {
        JwksError::Transport(msg) => assert!(msg.contains("exceeds cap")),
        other => panic!("expected Transport error naming the size cap, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_rejects_ip_literal_issuers() {
    let resolver = HttpJwksResolver::new();
    for iss in ["https://127.0.0.1", "https://10.0.0.8:8443", "https://[::1]"] {
        let err = resolver.resolve(iss).await.expect_err("IP literal must be refused");
        assert!(matches!(err, JwksError::Transport(_)), "{iss}: {err}");
    }
}

#[tokio::test]
async fn resolve_rejects_localhost_issuers() {
    let resolver = HttpJwksResolver::new();
    for iss in ["https://localhost", "https://foo.localhost"] {
        let err = resolver.resolve(iss).await.expect_err("loopback must be refused");
        assert!(matches!(err, JwksError::Transport(_)), "{iss}: {err}");
    }
}

#[tokio::test]
async fn resolve_rejects_issuers_outside_the_allowlist_before_any_fetch() {
    let resolver = HttpJwksResolver::new().with_allowed_issuers(["https://ap.example"]);
    let err = resolver
        .resolve("https://evil.example")
        .await
        .expect_err("unlisted issuer must be refused");
    assert!(matches!(err, JwksError::UnknownIssuer(_)));
}

#[test]
fn default_impl_delegates_to_new() {
    let resolver = HttpJwksResolver::default();
    assert_eq!(
        resolver.dwk_order.iter().map(WellKnownDwk::as_str).collect::<Vec<_>>(),
        vec![DWK_AGENT, DWK_RESOURCE, DWK_PERSON]
    );
}

#[tokio::test]
async fn resolve_from_base_aborts_streamed_body_over_cap_without_content_length() {
    // Set Transfer-Encoding: chunked explicitly so the response carries no
    // Content-Length header at all -- this exercises the streamed-read
    // abort path (the cap enforced per-chunk as bytes arrive), not the
    // upfront Content-Length check.
    let server = MockServer::start().await;
    let big_body = format!(r#"{{"keys": [], "padding": "{}"}}"#, "a".repeat(64));
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(big_body, "application/json")
                .append_header("Transfer-Encoding", "chunked"),
        )
        .mount(&server)
        .await;

    let resolver = HttpJwksResolver::with_config(
        default_dwk_order(),
        RequestTimeout::default(),
        MaxResponseBytes::new(8).unwrap(),
    );
    let err = resolver
        .resolve_from_base("https://issuer.example", &server.uri())
        .await
        .unwrap_err();
    match err {
        JwksError::Transport(msg) => assert!(msg.contains("response body exceeds cap")),
        other => panic!("expected Transport error naming the streamed body cap, got {other:?}"),
    }
}
