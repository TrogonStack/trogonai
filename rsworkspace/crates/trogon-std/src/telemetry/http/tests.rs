use super::*;
use trogon_semconv::attribute;
use tower::ServiceExt;

fn value_for<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a opentelemetry::Value> {
    attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .map(|attribute| &attribute.value)
}

#[tokio::test]
async fn instrument_router_executes_trace_callbacks() {
    let app =
        instrument_router(Router::<()>::new().route("/-/liveness", axum::routing::get(|| async { StatusCode::OK })));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/-/liveness")
                .version(Version::HTTP_11)
                .header(HOST, "gateway.test:8443")
                .header(USER_AGENT, "curl/8.7.1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn server_request_attributes_follow_http_semantic_conventions() {
    let request = Request::builder()
        .method("POST")
        .uri("/github/webhook")
        .version(Version::HTTP_11)
        .header(HOST, "gateway.test:8443")
        .header(USER_AGENT, "curl/8.7.1")
        .body(())
        .unwrap();

    let attributes = server_request_attributes(&request);

    assert_eq!(
        value_for(&attributes, attribute::HTTP_REQUEST_METHOD)
            .unwrap()
            .as_str()
            .as_ref(),
        "POST"
    );
    assert_eq!(
        value_for(&attributes, attribute::URL_PATH).unwrap().as_str().as_ref(),
        "/github/webhook"
    );
    assert_eq!(
        value_for(&attributes, attribute::NETWORK_PROTOCOL_VERSION)
            .unwrap()
            .as_str()
            .as_ref(),
        "1.1"
    );
    assert_eq!(
        value_for(&attributes, attribute::USER_AGENT_ORIGINAL)
            .unwrap()
            .as_str()
            .as_ref(),
        "curl/8.7.1"
    );
    assert_eq!(
        value_for(&attributes, attribute::SERVER_ADDRESS)
            .unwrap()
            .as_str()
            .as_ref(),
        "gateway.test"
    );
    assert_eq!(
        value_for(&attributes, attribute::SERVER_PORT),
        Some(&opentelemetry::Value::I64(8443))
    );
}

#[test]
fn server_request_attributes_skip_optional_headers_when_missing() {
    let request = Request::builder()
        .method("GET")
        .uri("/-/liveness")
        .version(Version::HTTP_2)
        .body(())
        .unwrap();

    let attributes = server_request_attributes(&request);

    assert_eq!(
        value_for(&attributes, attribute::HTTP_REQUEST_METHOD)
            .unwrap()
            .as_str()
            .as_ref(),
        "GET"
    );
    assert_eq!(
        value_for(&attributes, attribute::URL_PATH).unwrap().as_str().as_ref(),
        "/-/liveness"
    );
    assert_eq!(
        value_for(&attributes, attribute::NETWORK_PROTOCOL_VERSION)
            .unwrap()
            .as_str()
            .as_ref(),
        "2"
    );
    assert!(value_for(&attributes, attribute::USER_AGENT_ORIGINAL).is_none());
    assert!(value_for(&attributes, attribute::SERVER_ADDRESS).is_none());
    assert!(value_for(&attributes, attribute::SERVER_PORT).is_none());
}

#[test]
fn server_request_attributes_cover_other_protocol_versions() {
    let http_09_request = Request::builder()
        .method("GET")
        .uri("/legacy")
        .version(Version::HTTP_09)
        .body(())
        .unwrap();
    let http_10_request = Request::builder()
        .method("GET")
        .uri("/legacy")
        .version(Version::HTTP_10)
        .body(())
        .unwrap();
    let http_3_request = Request::builder()
        .method("GET")
        .uri("/modern")
        .version(Version::HTTP_3)
        .body(())
        .unwrap();

    assert_eq!(
        value_for(
            &server_request_attributes(&http_09_request),
            attribute::NETWORK_PROTOCOL_VERSION,
        )
        .unwrap()
        .as_str()
        .as_ref(),
        "0.9"
    );
    assert_eq!(
        value_for(
            &server_request_attributes(&http_10_request),
            attribute::NETWORK_PROTOCOL_VERSION,
        )
        .unwrap()
        .as_str()
        .as_ref(),
        "1.0"
    );
    assert_eq!(
        value_for(
            &server_request_attributes(&http_3_request),
            attribute::NETWORK_PROTOCOL_VERSION,
        )
        .unwrap()
        .as_str()
        .as_ref(),
        "3"
    );
}

#[test]
fn server_request_attributes_ignore_invalid_host_header() {
    let request = Request::builder()
        .method("GET")
        .uri("/-/liveness")
        .version(Version::HTTP_11)
        .header(HOST, "invalid host")
        .body(())
        .unwrap();

    let attributes = server_request_attributes(&request);

    assert!(value_for(&attributes, attribute::SERVER_ADDRESS).is_none());
    assert!(value_for(&attributes, attribute::SERVER_PORT).is_none());
}

#[test]
fn direct_span_attribute_helpers_do_not_panic() {
    let span = tracing::info_span!(trogon_semconv::span::HTTP_SERVER_REQUEST);
    let _guard = span.enter();
    let request = Request::builder()
        .method("GET")
        .uri("/-/ready")
        .version(Version::HTTP_11)
        .header(HOST, "gateway.test")
        .body(())
        .unwrap();

    set_server_request_span_attributes(&Span::current(), &request);
    set_server_response_span_attributes(&Span::current(), StatusCode::NO_CONTENT);
}

#[test]
fn server_response_attributes_follow_http_semantic_conventions() {
    let attributes = server_response_attributes(StatusCode::CREATED);

    assert_eq!(
        value_for(&attributes, attribute::HTTP_RESPONSE_STATUS_CODE),
        Some(&opentelemetry::Value::I64(201))
    );
}
