mod test_body;
mod test_telemetry;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, HeaderValue, Request, StatusCode, Version},
    routing::get,
};
use opentelemetry::{
    KeyValue,
    baggage::BaggageExt,
    propagation::Extractor,
    trace::{SpanKind, Status},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_semconv::attribute;

use super::header_extractor::HeaderExtractor;
use test_body::TestBody;
use test_telemetry::TestTelemetry;

fn attribute_value(attributes: &[KeyValue], name: &str) -> Option<String> {
    attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == name)
        .map(|attribute| attribute.value.to_string())
}

#[tokio::test]
async fn continues_w3c_trace_with_template_name_and_excludes_request_secrets() {
    let telemetry = TestTelemetry::new();
    let router = Router::new().route(
        "/items/{id}",
        get(|| async {
            assert!(tracing::Span::current().context().baggage().is_empty());
            StatusCode::NOT_FOUND
        }),
    );
    let request = Request::builder()
        .uri("/items/confidential-id?token=confidential-query")
        .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        .header("authorization", "Bearer confidential-authorization")
        .header("baggage", "customer=confidential-baggage")
        .header("host", "confidential-host")
        .header("user-agent", "confidential-user-agent")
        .body(Body::empty())
        .unwrap();
    let response = telemetry.request(router, request).await;
    telemetry.consume(response).await;

    let spans = telemetry.spans();
    assert_eq!(spans.len(), 1);
    let span = &spans[0];
    assert_eq!(span.name, "GET /items/{id}");
    assert_eq!(span.span_kind, SpanKind::Server);
    assert_eq!(
        span.span_context.trace_id().to_string(),
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert_eq!(span.parent_span_id.to_string(), "00f067aa0ba902b7");
    assert_eq!(span.status, Status::Unset);
    assert_eq!(
        attribute_value(&span.attributes, attribute::HTTP_ROUTE).as_deref(),
        Some("/items/{id}")
    );
    assert_eq!(
        attribute_value(&span.attributes, attribute::HTTP_RESPONSE_STATUS_CODE).as_deref(),
        Some("404")
    );
    assert_eq!(
        attribute_value(&span.attributes, attribute::URL_SCHEME).as_deref(),
        Some("http")
    );
    assert!(!format!("{span:?}").contains("confidential"));
    let points = telemetry.duration_points();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].count, 1);
    assert!(!format!("{:?}", points[0].attributes).contains("confidential"));
}

#[tokio::test]
async fn honors_unsampled_parent_and_still_records_metrics() {
    let telemetry = TestTelemetry::new();
    let request = Request::builder()
        .uri("/missing")
        .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
        .body(Body::empty())
        .unwrap();
    let response = telemetry.request(Router::new(), request).await;
    telemetry.consume(response).await;
    assert!(telemetry.spans().is_empty());
    assert_eq!(
        telemetry.duration_points().iter().map(|point| point.count).sum::<u64>(),
        1
    );
}

#[tokio::test]
async fn bounds_unknown_methods_and_unmatched_paths() {
    let telemetry = TestTelemetry::new();
    let request = Request::builder()
        .method("CUSTOM_PRIVATE_METHOD")
        .uri("/unknown/confidential-value")
        .body(Body::empty())
        .unwrap();
    let response = telemetry.request(Router::new(), request).await;
    telemetry.consume(response).await;
    let spans = telemetry.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].name, "_OTHER");
    assert_eq!(
        attribute_value(&spans[0].attributes, attribute::HTTP_REQUEST_METHOD).as_deref(),
        Some("_OTHER")
    );
    assert_eq!(attribute_value(&spans[0].attributes, attribute::HTTP_ROUTE), None);
    assert!(!format!("{:?}", spans[0]).contains("confidential"));
    let points = telemetry.duration_points();
    assert_eq!(
        attribute_value(&points[0].attributes, attribute::HTTP_REQUEST_METHOD).as_deref(),
        Some("_OTHER")
    );
    assert_eq!(attribute_value(&points[0].attributes, attribute::HTTP_ROUTE), None);
}

#[tokio::test]
async fn marks_server_failures_and_uses_seconds_histogram_buckets() {
    let telemetry = TestTelemetry::new();
    let router = Router::new().route("/failed", get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
    let response = telemetry
        .request(router, Request::builder().uri("/failed").body(Body::empty()).unwrap())
        .await;
    telemetry.consume(response).await;
    let spans = telemetry.spans();
    assert_eq!(spans[0].status, Status::error("503"));
    assert_eq!(
        attribute_value(&spans[0].attributes, attribute::ERROR_TYPE).as_deref(),
        Some("503")
    );
    let points = telemetry.duration_points();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].unit, "s");
    assert_eq!(
        points[0].bounds,
        [
            0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0
        ]
    );
    assert_eq!(
        attribute_value(&points[0].attributes, attribute::ERROR_TYPE).as_deref(),
        Some("503")
    );
}

#[tokio::test]
async fn excludes_every_operational_path_from_spans_and_metrics() {
    let telemetry = TestTelemetry::new();
    let router = trogon_provider_gateway::router(trogon_provider_gateway::Health::default());
    for path in ["/-/liveness", "/-/readiness", "/-/unknown"] {
        let request = Request::builder()
            .uri(path)
            .header("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .body(Body::empty())
            .unwrap();
        let response = telemetry.request(router.clone(), request).await;
        telemetry.consume(response).await;
    }
    assert!(telemetry.spans().is_empty());
    assert!(telemetry.duration_points().is_empty());
}

#[tokio::test]
async fn completes_streaming_request_after_body_not_response_headers() {
    let telemetry = TestTelemetry::new();
    let (release, body) = TestBody::streaming();
    let router = body.into_router();
    let response = telemetry.request(router, Request::new(Body::empty())).await;
    assert!(telemetry.spans().is_empty());
    assert!(telemetry.duration_points().is_empty());
    let mut consume = std::pin::pin!(telemetry.consume(response));
    assert!(futures_util::poll!(consume.as_mut()).is_pending());
    assert!(telemetry.spans().is_empty());
    assert!(telemetry.duration_points().is_empty());
    release.send(()).unwrap();
    consume.await;
    let spans = telemetry.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].status, Status::Unset);
    assert_eq!(
        telemetry.duration_points().iter().map(|point| point.count).sum::<u64>(),
        1
    );
}

#[tokio::test]
async fn body_failure_keeps_error_details_out_of_exported_attributes() {
    let telemetry = TestTelemetry::new();
    let router = Router::new().fallback(|| async { Body::new(TestBody::failing()) });
    let response = telemetry.request(router, Request::new(Body::empty())).await;
    assert!(to_bytes(response.into_body(), usize::MAX).await.is_err());
    let spans = telemetry.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].status, Status::error("response_body_error"));
    assert!(!format!("{:?}", spans[0]).contains("confidential"));
    let points = telemetry.duration_points();
    assert_eq!(points[0].count, 1);
    assert_eq!(
        attribute_value(&points[0].attributes, attribute::ERROR_TYPE).as_deref(),
        Some("response_body_error")
    );
}

#[tokio::test]
async fn abandoned_response_finishes_span_and_records_cancelled_request_once() {
    let telemetry = TestTelemetry::new();
    let (_release, body) = TestBody::streaming();
    let router = body.into_router();
    let response = telemetry.request(router, Request::new(Body::empty())).await;
    drop(response);
    let spans = telemetry.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].status, Status::error("request_cancelled"));
    let points = telemetry.duration_points();
    assert_eq!(points[0].count, 1);
    assert_eq!(
        attribute_value(&points[0].attributes, attribute::ERROR_TYPE).as_deref(),
        Some("request_cancelled")
    );
}

#[tokio::test]
async fn known_methods_reach_spans_metrics_and_completion_logs() {
    for method in [
        "GET", "POST", "HEAD", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
    ] {
        let telemetry = TestTelemetry::new();
        let request = Request::builder()
            .method(method)
            .uri("/missing")
            .body(Body::empty())
            .unwrap();
        let response = telemetry.request(Router::new(), request).await;
        telemetry.consume(response).await;
        let spans = telemetry.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name.as_ref(), method);
        assert_eq!(
            attribute_value(&spans[0].attributes, attribute::HTTP_REQUEST_METHOD).as_deref(),
            Some(method)
        );
        let points = telemetry.duration_points();
        assert_eq!(
            attribute_value(&points[0].attributes, attribute::HTTP_REQUEST_METHOD).as_deref(),
            Some(method)
        );
        let events = telemetry.events();
        let event = events
            .iter()
            .find(|event| event.message() == Some("HTTP request completed"))
            .unwrap();
        assert_eq!(event.field("method"), Some(method));
        assert_eq!(event.field("status_code"), Some("404"));
        assert_eq!(event.field("error_type"), None);
        assert!(event.field("duration_secs").unwrap().parse::<f64>().unwrap() >= 0.0);
    }
}

#[tokio::test]
async fn protocol_versions_are_preserved_in_spans_and_metrics() {
    for (version, expected) in [
        (Version::HTTP_09, "0.9"),
        (Version::HTTP_10, "1.0"),
        (Version::HTTP_11, "1.1"),
        (Version::HTTP_2, "2"),
        (Version::HTTP_3, "3"),
    ] {
        let telemetry = TestTelemetry::new();
        let request = Request::builder()
            .version(version)
            .uri("/missing")
            .body(Body::empty())
            .unwrap();
        let response = telemetry.request(Router::new(), request).await;
        telemetry.consume(response).await;
        let spans = telemetry.spans();
        assert_eq!(
            attribute_value(&spans[0].attributes, attribute::NETWORK_PROTOCOL_VERSION).as_deref(),
            Some(expected)
        );
        let points = telemetry.duration_points();
        assert_eq!(
            attribute_value(&points[0].attributes, attribute::NETWORK_PROTOCOL_VERSION).as_deref(),
            Some(expected)
        );
    }
}

#[tokio::test]
async fn failed_requests_keep_their_status_and_error_in_completion_log_fields() {
    let telemetry = TestTelemetry::new();
    let router = Router::new().fallback(|| async { StatusCode::SERVICE_UNAVAILABLE });
    let response = telemetry.request(router, Request::new(Body::empty())).await;
    telemetry.consume(response).await;
    let events = telemetry.events();
    let event = events
        .iter()
        .find(|event| event.message() == Some("HTTP request completed"))
        .unwrap();
    assert_eq!(event.field("method"), Some("GET"));
    assert_eq!(event.field("status_code"), Some("503"));
    assert_eq!(event.field("error_type"), Some("503"));
}

#[test]
fn header_carrier_exposes_keys_and_rejects_non_text_values() {
    let mut headers = HeaderMap::new();
    headers.insert("traceparent", HeaderValue::from_static("opaque-context"));
    headers.insert("authorization", HeaderValue::from_bytes(&[0xff]).unwrap());
    let carrier = HeaderExtractor(&headers);
    let mut keys = carrier.keys();
    keys.sort_unstable();
    assert_eq!(keys, ["authorization", "traceparent"]);
    assert_eq!(carrier.get("traceparent"), Some("opaque-context"));
    assert_eq!(carrier.get("authorization"), None);
    assert_eq!(carrier.get("missing"), None);
}
