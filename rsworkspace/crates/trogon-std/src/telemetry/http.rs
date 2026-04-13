use axum::{
    Router,
    body::Body,
    http::{
        Request, StatusCode, Version,
        header::{HOST, USER_AGENT},
        uri::Authority,
    },
};
use opentelemetry::KeyValue;
use opentelemetry_semantic_conventions::trace as semconv;
use tower_http::trace::TraceLayer;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub fn instrument_router<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &Request<Body>| {
                let span = tracing::info_span!(
                    "http.server.request",
                    method = %request.method(),
                    path = %request.uri().path()
                );
                set_server_request_span_attributes(&span, request);
                span
            })
            .on_response(
                |response: &axum::http::Response<Body>,
                 _latency: std::time::Duration,
                 span: &tracing::Span| {
                    set_server_response_span_attributes(span, response.status());
                },
            ),
    )
}

pub fn server_request_attributes<B>(request: &Request<B>) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(
            semconv::HTTP_REQUEST_METHOD,
            request.method().as_str().to_owned(),
        ),
        KeyValue::new(semconv::URL_PATH, request.uri().path().to_owned()),
    ];

    if let Some(protocol_version) = protocol_version(request.version()) {
        attributes.push(KeyValue::new(
            semconv::NETWORK_PROTOCOL_VERSION,
            protocol_version,
        ));
    }

    if let Some(user_agent) = request
        .headers()
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
    {
        attributes.push(KeyValue::new(
            semconv::USER_AGENT_ORIGINAL,
            user_agent.to_owned(),
        ));
    }

    if let Some(authority) = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok())
    {
        attributes.push(KeyValue::new(
            semconv::SERVER_ADDRESS,
            authority.host().to_owned(),
        ));
        if let Some(port) = authority.port_u16() {
            attributes.push(KeyValue::new(semconv::SERVER_PORT, i64::from(port)));
        }
    }

    attributes
}

pub fn set_server_request_span_attributes<B>(span: &Span, request: &Request<B>) {
    for attribute in server_request_attributes(request) {
        span.set_attribute(attribute.key, attribute.value);
    }
}

pub fn server_response_attributes(status: StatusCode) -> [KeyValue; 1] {
    [KeyValue::new(
        semconv::HTTP_RESPONSE_STATUS_CODE,
        i64::from(status.as_u16()),
    )]
}

pub fn set_server_response_span_attributes(span: &Span, status: StatusCode) {
    for attribute in server_response_attributes(status) {
        span.set_attribute(attribute.key, attribute.value);
    }
}

#[rustfmt::skip]
fn protocol_version(version: Version) -> Option<&'static str> {
    Some(if version == Version::HTTP_09 { "0.9" } else if version == Version::HTTP_10 { "1.0" } else if version == Version::HTTP_11 { "1.1" } else if version == Version::HTTP_2 { "2" } else if version == Version::HTTP_3 { "3" } else { return None; })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn value_for<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a opentelemetry::Value> {
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| &attribute.value)
    }

    #[tokio::test]
    async fn instrument_router_executes_trace_callbacks() {
        let app = instrument_router(Router::<()>::new().route(
            "/-/liveness",
            axum::routing::get(|| async { StatusCode::OK }),
        ));

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
            value_for(&attributes, semconv::HTTP_REQUEST_METHOD)
                .unwrap()
                .as_str()
                .as_ref(),
            "POST"
        );
        assert_eq!(
            value_for(&attributes, semconv::URL_PATH)
                .unwrap()
                .as_str()
                .as_ref(),
            "/github/webhook"
        );
        assert_eq!(
            value_for(&attributes, semconv::NETWORK_PROTOCOL_VERSION)
                .unwrap()
                .as_str()
                .as_ref(),
            "1.1"
        );
        assert_eq!(
            value_for(&attributes, semconv::USER_AGENT_ORIGINAL)
                .unwrap()
                .as_str()
                .as_ref(),
            "curl/8.7.1"
        );
        assert_eq!(
            value_for(&attributes, semconv::SERVER_ADDRESS)
                .unwrap()
                .as_str()
                .as_ref(),
            "gateway.test"
        );
        assert_eq!(
            value_for(&attributes, semconv::SERVER_PORT)
                .unwrap()
                .as_str()
                .as_ref(),
            "8443"
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
            value_for(&attributes, semconv::HTTP_REQUEST_METHOD)
                .unwrap()
                .as_str()
                .as_ref(),
            "GET"
        );
        assert_eq!(
            value_for(&attributes, semconv::URL_PATH)
                .unwrap()
                .as_str()
                .as_ref(),
            "/-/liveness"
        );
        assert_eq!(
            value_for(&attributes, semconv::NETWORK_PROTOCOL_VERSION)
                .unwrap()
                .as_str()
                .as_ref(),
            "2"
        );
        assert!(value_for(&attributes, semconv::USER_AGENT_ORIGINAL).is_none());
        assert!(value_for(&attributes, semconv::SERVER_ADDRESS).is_none());
        assert!(value_for(&attributes, semconv::SERVER_PORT).is_none());
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
                semconv::NETWORK_PROTOCOL_VERSION,
            )
            .unwrap()
            .as_str()
            .as_ref(),
            "0.9"
        );
        assert_eq!(
            value_for(
                &server_request_attributes(&http_10_request),
                semconv::NETWORK_PROTOCOL_VERSION,
            )
            .unwrap()
            .as_str()
            .as_ref(),
            "1.0"
        );
        assert_eq!(
            value_for(
                &server_request_attributes(&http_3_request),
                semconv::NETWORK_PROTOCOL_VERSION,
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

        assert!(value_for(&attributes, semconv::SERVER_ADDRESS).is_none());
        assert!(value_for(&attributes, semconv::SERVER_PORT).is_none());
    }

    #[test]
    fn direct_span_attribute_helpers_do_not_panic() {
        let span = tracing::info_span!("http.server.request.test");
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
            value_for(&attributes, semconv::HTTP_RESPONSE_STATUS_CODE)
                .unwrap()
                .as_str()
                .as_ref(),
            "201"
        );
    }
}
