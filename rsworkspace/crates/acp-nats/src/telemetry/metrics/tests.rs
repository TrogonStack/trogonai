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
    for method in &["initialize", "authenticate", "new_session", "load_session", "prompt"] {
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
