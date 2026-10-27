mod header_extractor;
mod request_completion;
mod request_method;
mod request_record;
mod response_body;

use axum::{
    Router,
    extract::{MatchedPath, Request, State},
    http::Version,
    middleware::{self, Next},
    response::Response,
};
use opentelemetry::{Context, KeyValue, metrics::Histogram, propagation::TextMapPropagator};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_semconv::{attribute, metric, span};

use header_extractor::HeaderExtractor;
use request_method::RequestMethod;
use request_record::RequestRecord;
use response_body::ResponseBody;

#[derive(Clone)]
pub(crate) struct HttpTelemetry {
    duration: Histogram<f64>,
}

impl HttpTelemetry {
    pub(crate) fn new(meter: opentelemetry::metrics::Meter) -> Self {
        Self {
            duration: metric::build_http_server_request_duration(&meter),
        }
    }

    pub(crate) fn instrument(&self, router: Router) -> Router {
        router.layer(middleware::from_fn_with_state(self.clone(), observe))
    }
}

async fn observe(State(telemetry): State<HttpTelemetry>, request: Request, next: Next) -> Response {
    if request.uri().path().starts_with("/-/") {
        return next.run(request).await;
    }

    let method = RequestMethod::from(request.method());
    let route = request.extensions().get::<MatchedPath>();
    let name = route.map_or_else(
        || method.as_str().to_owned(),
        |route| format!("{} {}", method.as_str(), route.as_str()),
    );
    let span = tracing::info_span!(parent: None, span::HTTP_SERVER_REQUEST, otel.name = %name, otel.kind = "server");
    let parent =
        TraceContextPropagator::new().extract_with_context(&Context::new(), &HeaderExtractor(request.headers()));
    let _ = span.set_parent(parent);

    let mut attributes = vec![
        KeyValue::new(attribute::HTTP_REQUEST_METHOD, method.as_str()),
        KeyValue::new(attribute::URL_SCHEME, "http"),
    ];
    if let Some(route) = route {
        attributes.push(KeyValue::new(attribute::HTTP_ROUTE, route.as_str().to_owned()));
    }
    if let Some(version) = protocol_version(request.version()) {
        attributes.push(KeyValue::new(attribute::NETWORK_PROTOCOL_VERSION, version));
    }
    for attribute in &attributes {
        span.set_attribute(attribute.key.clone(), attribute.value.clone());
    }

    let mut record = RequestRecord::new(span.clone(), telemetry.duration, attributes, method);
    async move {
        let response = next.run(request).await;
        record.set_response_status(response.status());
        response.map(|body| axum::body::Body::new(ResponseBody::new(body, record)))
    }
    .instrument(span)
    .await
}

fn protocol_version(version: Version) -> Option<&'static str> {
    match version {
        Version::HTTP_09 => Some("0.9"),
        Version::HTTP_10 => Some("1.0"),
        Version::HTTP_11 => Some("1.1"),
        Version::HTTP_2 => Some("2"),
        Version::HTTP_3 => Some("3"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
