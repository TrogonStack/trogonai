use super::*;
use crate::telemetry::metrics::Metrics;
use trogon_semconv::metric;

#[test]
fn has_request_metric_matches_correct_method_and_success() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_request("initialize", 0.01, true);

    let finished = flush_metrics(&provider, &exporter);
    assert!(has_request_metric(&finished, "initialize", true));
    provider.shutdown().unwrap();
}

#[test]
fn has_request_metric_rejects_wrong_method() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_request("initialize", 0.01, true);

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_request_metric(&finished, "authenticate", true));
    provider.shutdown().unwrap();
}

#[test]
fn has_request_metric_rejects_wrong_success() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_request("initialize", 0.01, true);

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_request_metric(&finished, "initialize", false));
    provider.shutdown().unwrap();
}

#[test]
fn has_request_metric_returns_false_when_empty() {
    let (provider, exporter) = test_provider();
    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_request_metric(&finished, "initialize", true));
    provider.shutdown().unwrap();
}

#[test]
// Builds a histogram under a counter's name to prove the lookup rejects the
// wrong instrument kind; that mismatch is the point, so the generated
// `build_acp_requests` (a counter) cannot stand in here.
#[cfg_attr(dylint_lib = "trogon_lints", allow(telemetry_metric_construction))]
fn has_request_metric_returns_false_for_histogram_metric() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let histogram = meter
        .f64_histogram(metric::ACP_REQUESTS)
        .with_description("test")
        .build();
    histogram.record(1.0, &[]);

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_request_metric(&finished, "initialize", true));
    provider.shutdown().unwrap();
}

#[test]
fn has_error_metric_matches_correct_operation_and_reason() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_error("session_validate", "invalid_session_id");

    let finished = flush_metrics(&provider, &exporter);
    assert!(has_error_metric(&finished, "session_validate", "invalid_session_id"));
    provider.shutdown().unwrap();
}

#[test]
fn has_error_metric_rejects_wrong_operation() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_error("session_validate", "invalid_session_id");

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_error_metric(&finished, "wrong_operation", "invalid_session_id"));
    provider.shutdown().unwrap();
}

#[test]
fn has_error_metric_rejects_wrong_reason() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_error("session_validate", "invalid_session_id");

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_error_metric(&finished, "session_validate", "wrong_reason"));
    provider.shutdown().unwrap();
}

#[test]
// Builds a histogram under a counter's name to prove the lookup rejects the
// wrong instrument kind; that mismatch is the point, so the generated
// `build_acp_errors` (a counter) cannot stand in here.
#[cfg_attr(dylint_lib = "trogon_lints", allow(telemetry_metric_construction))]
fn has_error_metric_returns_false_for_histogram_metric() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let histogram = meter.f64_histogram(metric::ACP_ERRORS).with_description("test").build();
    histogram.record(1.0, &[]);

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_error_metric(&finished, "session_validate", "invalid_session_id"));
    provider.shutdown().unwrap();
}

#[test]
fn has_session_ready_error_metric_matches() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_error("session_ready", "session_ready_publish_failed");

    let finished = flush_metrics(&provider, &exporter);
    assert!(has_session_ready_error_metric(&finished));
    provider.shutdown().unwrap();
}

#[test]
fn has_session_ready_error_metric_rejects_wrong_operation() {
    let (provider, exporter) = test_provider();
    let meter = provider.meter("test");
    let metrics = Metrics::new(&meter);
    metrics.record_error("wrong_operation", "session_ready_publish_failed");

    let finished = flush_metrics(&provider, &exporter);
    assert!(!has_session_ready_error_metric(&finished));
    provider.shutdown().unwrap();
}
