use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

#[derive(Clone)]
pub struct Metrics {
    requests: Counter<u64>,
    request_duration: Histogram<f64>,
    errors: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            requests: meter
                .u64_counter("acp.requests")
                .with_description("Total number of ACP requests")
                .build(),
            request_duration: meter
                .f64_histogram("acp.request.duration")
                .with_description("Duration of ACP requests in seconds")
                .with_unit("s")
                .build(),
            errors: meter
                .u64_counter("acp.errors")
                .with_description("Total number of errors by operation and reason")
                .build(),
        }
    }

    pub fn record_request(&self, method: &'static str, duration: f64, success: bool) {
        let attrs = &[KeyValue::new("method", method), KeyValue::new("success", success)];
        self.requests.add(1, attrs);
        self.request_duration.record(duration, attrs);
    }

    pub fn record_error(&self, operation: &'static str, reason: &'static str) {
        self.errors.add(
            1,
            &[KeyValue::new("operation", operation), KeyValue::new("reason", reason)],
        );
    }
}

#[cfg(test)]
mod tests;
