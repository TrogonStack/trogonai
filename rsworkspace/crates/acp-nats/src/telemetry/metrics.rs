use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, Meter};

#[derive(Clone)]
pub struct Metrics {
    requests_total: Counter<u64>,
    request_duration: Histogram<f64>,
    errors: Counter<u64>,
}

impl Metrics {
    pub fn new(meter: &Meter) -> Self {
        Self {
            requests_total: meter
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
        let attrs = &[
            KeyValue::new("method", method),
            KeyValue::new("success", success),
        ];
        self.requests_total.add(1, attrs);
        self.request_duration.record(duration, attrs);
    }

    pub fn record_error(&self, operation: &'static str, reason: &'static str) {
        self.errors.add(
            1,
            &[
                KeyValue::new("operation", operation),
                KeyValue::new("reason", reason),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meter() -> opentelemetry::metrics::Meter {
        opentelemetry::global::meter("acp-nats-metrics-test")
    }

    #[test]
    fn new_constructs_without_panic() {
        let _ = Metrics::new(&test_meter());
    }

    #[test]
    fn record_request_success_does_not_panic() {
        Metrics::new(&test_meter()).record_request("initialize", 0.042, true);
    }

    #[test]
    fn record_request_failure_does_not_panic() {
        Metrics::new(&test_meter()).record_request("prompt", 1.5, false);
    }

    #[test]
    fn record_error_does_not_panic() {
        Metrics::new(&test_meter()).record_error("initialize", "agent_unavailable");
    }

    #[test]
    fn record_request_all_methods_do_not_panic() {
        let m = Metrics::new(&test_meter());
        for method in &[
            "initialize",
            "authenticate",
            "new_session",
            "load_session",
            "prompt",
        ] {
            m.record_request(method, 0.001, true);
        }
    }

    #[test]
    fn metrics_clone_is_usable() {
        let m = Metrics::new(&test_meter());
        let m2 = m.clone();
        m.record_request("ping", 0.0, true);
        m2.record_error("ping", "test");
    }
}
