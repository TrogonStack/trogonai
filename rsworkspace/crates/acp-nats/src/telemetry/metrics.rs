use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};
use trogon_semconv::{attribute, metric};

#[derive(Clone)]
pub struct Metrics {
    requests: Counter<u64>,
    request_duration: Histogram<f64>,
    errors: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            requests: metric::build_acp_requests(meter),
            request_duration: metric::build_acp_request_duration(meter),
            errors: metric::build_acp_errors(meter),
        }
    }

    pub fn record_request(&self, method: &'static str, duration: f64, success: bool) {
        let attrs = &[
            KeyValue::new(attribute::METHOD, method),
            KeyValue::new(attribute::SUCCESS, success),
        ];
        self.requests.add(1, attrs);
        self.request_duration.record(duration, attrs);
    }

    pub fn record_error(&self, operation: &'static str, reason: &'static str) {
        self.errors.add(
            1,
            &[
                KeyValue::new(attribute::OPERATION, operation),
                KeyValue::new(attribute::REASON, reason),
            ],
        );
    }
}

#[cfg(test)]
mod tests;
