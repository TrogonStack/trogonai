use std::time::Instant;

use axum::http::StatusCode;
use opentelemetry::{KeyValue, metrics::Histogram, trace::Status};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_semconv::attribute;

use super::{request_completion::RequestCompletion, request_method::RequestMethod};

pub(super) struct RequestRecord {
    pub(super) span: Span,
    duration: Option<Histogram<f64>>,
    started: Instant,
    attributes: Vec<KeyValue>,
    method: RequestMethod,
    status: Option<StatusCode>,
}

impl RequestRecord {
    pub(super) fn new(span: Span, duration: Histogram<f64>, attributes: Vec<KeyValue>, method: RequestMethod) -> Self {
        Self {
            span,
            duration: Some(duration),
            started: Instant::now(),
            attributes,
            method,
            status: None,
        }
    }

    pub(super) fn set_response_status(&mut self, status: StatusCode) {
        self.status = Some(status);
        let value = i64::from(status.as_u16());
        self.span.set_attribute(attribute::HTTP_RESPONSE_STATUS_CODE, value);
        self.attributes
            .push(KeyValue::new(attribute::HTTP_RESPONSE_STATUS_CODE, value));
    }

    pub(super) fn finish(mut self, completion: RequestCompletion) {
        self.record(completion);
    }

    fn record(&mut self, completion: RequestCompletion) {
        let Some(duration) = self.duration.take() else {
            return;
        };
        let error_type = completion.error_type().map(str::to_owned).or_else(|| {
            self.status
                .filter(|status| status.is_server_error())
                .map(|status| status.as_u16().to_string())
        });
        if let Some(error_type) = &error_type {
            self.span.set_status(Status::error(error_type.clone()));
            self.span.set_attribute(attribute::ERROR_TYPE, error_type.clone());
            self.attributes
                .push(KeyValue::new(attribute::ERROR_TYPE, error_type.clone()));
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        let _entered = self.span.enter();
        let _context = self.span.context().attach();
        duration.record(elapsed, &self.attributes);
        tracing::info!(
            method = self.method.as_str(),
            status_code = self.status.map(|status| status.as_u16()),
            error_type = error_type.as_deref(),
            duration_secs = elapsed,
            "HTTP request completed"
        );
    }
}

impl Drop for RequestRecord {
    fn drop(&mut self) {
        self.record(RequestCompletion::Cancelled);
    }
}
