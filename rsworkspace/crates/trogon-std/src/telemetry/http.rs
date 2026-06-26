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
use tower_http::trace::TraceLayer;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_semconv::{attribute, span};

pub fn instrument_router<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &Request<Body>| {
                let span = tracing::info_span!(
                    span::HTTP_SERVER_REQUEST,
                    otel.kind = "server",
                    method = %request.method(),
                    path = %request.uri().path()
                );
                set_server_request_span_attributes(&span, request);
                span
            })
            .on_response(
                |response: &axum::http::Response<Body>, _latency: std::time::Duration, span: &tracing::Span| {
                    set_server_response_span_attributes(span, response.status());
                },
            ),
    )
}

pub fn server_request_attributes<B>(request: &Request<B>) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(attribute::HTTP_REQUEST_METHOD, request.method().as_str().to_owned()),
        KeyValue::new(attribute::URL_PATH, request.uri().path().to_owned()),
    ];

    if let Some(protocol_version) = protocol_version(request.version()) {
        attributes.push(KeyValue::new(attribute::NETWORK_PROTOCOL_VERSION, protocol_version));
    }

    if let Some(user_agent) = request.headers().get(USER_AGENT).and_then(|value| value.to_str().ok()) {
        attributes.push(KeyValue::new(attribute::USER_AGENT_ORIGINAL, user_agent.to_owned()));
    }

    if let Some(authority) = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok())
    {
        attributes.push(KeyValue::new(attribute::SERVER_ADDRESS, authority.host().to_owned()));
        if let Some(port) = authority.port_u16() {
            attributes.push(KeyValue::new(attribute::SERVER_PORT, i64::from(port)));
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
        attribute::HTTP_RESPONSE_STATUS_CODE,
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
mod tests;
