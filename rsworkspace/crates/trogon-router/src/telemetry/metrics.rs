//! OpenTelemetry metrics for the Router Agent.
//!
//! Instruments are created lazily on first use via [`std::sync::OnceLock`] and
//! record to whichever meter provider was installed by `trogon_telemetry::init_logger`.
//! In binaries that call `init_logger` the metrics are exported to the OTLP
//! endpoint.  In tests (where no provider is installed) the calls are no-ops.

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::sync::OnceLock;

// ── Meter ─────────────────────────────────────────────────────────────────────

fn meter() -> opentelemetry::metrics::Meter {
    opentelemetry::global::meter("trogon-router")
}

// ── Instruments ───────────────────────────────────────────────────────────────

static LLM_LATENCY_MS: OnceLock<Histogram<f64>> = OnceLock::new();
static EVENTS_ROUTED: OnceLock<Counter<u64>> = OnceLock::new();
static EVENTS_UNROUTABLE: OnceLock<Counter<u64>> = OnceLock::new();
static EVENTS_ERROR: OnceLock<Counter<u64>> = OnceLock::new();
static DLQ_PUBLISHED: OnceLock<Counter<u64>> = OnceLock::new();

fn llm_latency_ms() -> &'static Histogram<f64> {
    LLM_LATENCY_MS.get_or_init(|| {
        meter()
            .f64_histogram("trogon.router.llm.latency_ms")
            .with_description("LLM routing call latency in milliseconds")
            .with_unit("ms")
            .build()
    })
}

fn events_routed() -> &'static Counter<u64> {
    EVENTS_ROUTED.get_or_init(|| {
        meter()
            .u64_counter("trogon.router.events.routed")
            .with_description("Number of events successfully routed to an actor")
            .build()
    })
}

fn events_unroutable() -> &'static Counter<u64> {
    EVENTS_UNROUTABLE.get_or_init(|| {
        meter()
            .u64_counter("trogon.router.events.unroutable")
            .with_description("Number of events the LLM could not route to any actor")
            .build()
    })
}

fn events_error() -> &'static Counter<u64> {
    EVENTS_ERROR.get_or_init(|| {
        meter()
            .u64_counter("trogon.router.events.error")
            .with_description("Number of events that caused a routing error")
            .build()
    })
}

fn dlq_published() -> &'static Counter<u64> {
    DLQ_PUBLISHED.get_or_init(|| {
        meter()
            .u64_counter("trogon.router.dlq.published")
            .with_description("Number of unroutable events published to the dead-letter queue")
            .build()
    })
}

// ── Public recording helpers ──────────────────────────────────────────────────

/// Record the latency of a single LLM call and the event type it was for.
pub fn record_llm_latency(event_type: &str, latency_ms: f64) {
    llm_latency_ms().record(
        latency_ms,
        &[KeyValue::new("event_type", event_type.to_string())],
    );
}

/// Increment the routed-events counter.
pub fn inc_events_routed(event_type: &str, agent_type: &str) {
    events_routed().add(
        1,
        &[
            KeyValue::new("event_type", event_type.to_string()),
            KeyValue::new("agent_type", agent_type.to_string()),
        ],
    );
}

/// Increment the unroutable-events counter.
pub fn inc_events_unroutable(event_type: &str) {
    events_unroutable().add(1, &[KeyValue::new("event_type", event_type.to_string())]);
}

/// Increment the routing-error counter.
pub fn inc_events_error(event_type: &str, error_kind: &str) {
    events_error().add(
        1,
        &[
            KeyValue::new("event_type", event_type.to_string()),
            KeyValue::new("error_kind", error_kind.to_string()),
        ],
    );
}

/// Increment the DLQ-published counter.
pub fn inc_dlq_published(event_type: &str) {
    dlq_published().add(1, &[KeyValue::new("event_type", event_type.to_string())]);
}
