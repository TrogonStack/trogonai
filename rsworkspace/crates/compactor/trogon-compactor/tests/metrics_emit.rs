//! S4 integration test: verify the compaction telemetry hook emits the expected
//! instruments and attributes on all three paths — success, fallback, and error.
//!
//! Runs as its own test binary (a separate process from `--lib`), so the lazily
//! initialized `OnceLock` instruments in `telemetry::metrics` bind to the
//! in-memory `SdkMeterProvider` installed here rather than to any provider a unit
//! test might install first. Kept as a single `#[test]` for the same reason: all
//! `record_*` calls must observe the same global provider.

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};

use trogon_compactor::telemetry::metrics::{
    CompactionIdentity, record_compaction, record_failure, record_request,
};

/// True if `metrics` contains an instrument named `name` with at least one data
/// point whose attribute set is a superset of `expected` (`key` == `value`).
fn has_instrument_with_attrs(
    metrics: &[ResourceMetrics],
    name: &str,
    expected: &[(&str, &str)],
) -> bool {
    for rm in metrics {
        for sm in rm.scope_metrics() {
            for metric in sm.metrics() {
                if metric.name() != name {
                    continue;
                }
                let point_attr_sets: Vec<Vec<KeyValue>> = match metric.data() {
                    AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                        .data_points()
                        .map(|dp| dp.attributes().cloned().collect())
                        .collect(),
                    AggregatedMetrics::U64(MetricData::Histogram(h)) => h
                        .data_points()
                        .map(|dp| dp.attributes().cloned().collect())
                        .collect(),
                    _ => Vec::new(),
                };
                for attrs in &point_attr_sets {
                    let all_present = expected.iter().all(|(k, v)| {
                        attrs
                            .iter()
                            .any(|kv| kv.key.as_str() == *k && kv.value.as_str() == *v)
                    });
                    if all_present {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn instrument_present(metrics: &[ResourceMetrics], name: &str) -> bool {
    metrics
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .any(|m| m.name() == name)
}

#[test]
fn emits_metrics_on_success_fallback_and_error_paths() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    global::set_meter_provider(provider.clone());

    // Every request increments requests_total on entry.
    record_request();
    record_request();
    record_request();

    let success_identity = CompactionIdentity {
        session_provider: "anthropic",
        session_model: "claude-session",
        compactor_provider: "anthropic",
        compactor_model: "claude-haiku",
    };

    // 1) Success path: used pair == compactor pair, no fallback.
    record_compaction(
        "anthropic",
        "claude-haiku",
        success_identity,
        true,
        1_000,
        400,
        12,
        None,
    );

    // 2) Fallback path: chosen compactor failed, used pair becomes the session
    //    pair; fallback_model + fallback_reason are emitted.
    let fallback_identity = CompactionIdentity {
        session_provider: "anthropic",
        session_model: "claude-session",
        compactor_provider: "xai",
        compactor_model: "grok-fast",
    };
    record_compaction(
        "anthropic",
        "claude-session",
        fallback_identity,
        true,
        1_000,
        500,
        34,
        Some(("claude-session", "http")),
    );

    // 3) Error path: failure counter with error_kind + the four identity attrs.
    let error_identity = CompactionIdentity {
        session_provider: "anthropic",
        session_model: "claude-session",
        compactor_provider: "xai",
        compactor_model: "grok-fast",
    };
    record_failure("xai", "grok-fast", error_identity, "empty_response");

    provider.force_flush().expect("force_flush");
    let metrics = exporter
        .get_finished_metrics()
        .expect("metrics should be exported");

    // requests_total counted every request entry.
    assert!(
        instrument_present(&metrics, "trogon.compactor.requests_total"),
        "requests_total must be emitted"
    );

    // Success: success_total + token usage histograms with the four identity attrs.
    assert!(
        has_instrument_with_attrs(
            &metrics,
            "trogon.compactor.success_total",
            &[
                ("session_provider", "anthropic"),
                ("session_model", "claude-session"),
                ("compactor_provider", "anthropic"),
                ("compactor_model", "claude-haiku"),
            ],
        ),
        "success_total must carry the four explicit identity attributes"
    );
    assert!(
        instrument_present(&metrics, "gen_ai.client.token.usage"),
        "gen_ai token-usage histogram must be emitted"
    );
    assert!(
        instrument_present(&metrics, "trogon.compactor.compression_ratio"),
        "compression_ratio histogram must be emitted"
    );
    assert!(
        instrument_present(&metrics, "trogon.compactor.duration"),
        "duration histogram must be emitted"
    );

    // Fallback: fallback_total with fallback_model + fallback_reason, and
    // compactor_* reflecting the originally-chosen (failed) pair.
    assert!(
        has_instrument_with_attrs(
            &metrics,
            "trogon.compactor.fallback_total",
            &[
                ("fallback_model", "claude-session"),
                ("fallback_reason", "http"),
                ("compactor_provider", "xai"),
                ("compactor_model", "grok-fast"),
                ("session_provider", "anthropic"),
                ("session_model", "claude-session"),
            ],
        ),
        "fallback_total must carry fallback_model, fallback_reason and the four identity attrs"
    );

    // Error: failure_total with error_kind + the four identity attrs.
    assert!(
        has_instrument_with_attrs(
            &metrics,
            "trogon.compactor.failure_total",
            &[
                ("error_kind", "empty_response"),
                ("session_provider", "anthropic"),
                ("session_model", "claude-session"),
                ("compactor_provider", "xai"),
                ("compactor_model", "grok-fast"),
            ],
        ),
        "failure_total must carry error_kind and the four identity attributes"
    );
}
