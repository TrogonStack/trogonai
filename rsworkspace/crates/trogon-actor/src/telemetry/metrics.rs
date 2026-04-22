//! OpenTelemetry metrics for Entity Actors.
//!
//! Instruments record per-actor-type dimensions so dashboards can break down
//! latency and error rates by actor type.

use opentelemetry::{
    KeyValue,
    metrics::{Counter, Histogram},
};
use std::sync::OnceLock;

// ── Meter ─────────────────────────────────────────────────────────────────────

fn meter() -> opentelemetry::metrics::Meter {
    opentelemetry::global::meter("trogon-actor")
}

// ── Instruments ───────────────────────────────────────────────────────────────

static HANDLE_LATENCY_MS: OnceLock<Histogram<f64>> = OnceLock::new();
static OCC_RETRIES: OnceLock<Counter<u64>> = OnceLock::new();
static SPAWN_CALLS: OnceLock<Counter<u64>> = OnceLock::new();
static SPAWN_ERRORS: OnceLock<Counter<u64>> = OnceLock::new();
static HANDLE_ERRORS: OnceLock<Counter<u64>> = OnceLock::new();
static HANDLE_TIMEOUTS: OnceLock<Counter<u64>> = OnceLock::new();

fn handle_latency_ms() -> &'static Histogram<f64> {
    HANDLE_LATENCY_MS.get_or_init(|| {
        meter()
            .f64_histogram("trogon.actor.handle.latency_ms")
            .with_description("handle_event end-to-end latency in milliseconds")
            .with_unit("ms")
            .build()
    })
}

fn occ_retries() -> &'static Counter<u64> {
    OCC_RETRIES.get_or_init(|| {
        meter()
            .u64_counter("trogon.actor.handle.occ_retries")
            .with_description("Optimistic concurrency conflict retries in handle_event")
            .build()
    })
}

fn spawn_calls() -> &'static Counter<u64> {
    SPAWN_CALLS.get_or_init(|| {
        meter()
            .u64_counter("trogon.actor.spawn.calls")
            .with_description("Number of spawn_agent calls")
            .build()
    })
}

fn spawn_errors() -> &'static Counter<u64> {
    SPAWN_ERRORS.get_or_init(|| {
        meter()
            .u64_counter("trogon.actor.spawn.errors")
            .with_description(
                "Number of spawn_agent failures (no capability, timeout, request error)",
            )
            .build()
    })
}

fn handle_errors() -> &'static Counter<u64> {
    HANDLE_ERRORS.get_or_init(|| {
        meter()
            .u64_counter("trogon.actor.handle.errors")
            .with_description("Number of handle_event invocations that returned an error")
            .build()
    })
}

fn handle_timeouts() -> &'static Counter<u64> {
    HANDLE_TIMEOUTS.get_or_init(|| {
        meter()
            .u64_counter("trogon.actor.handle.timeouts")
            .with_description("Number of handle_event invocations cancelled by the event timeout")
            .build()
    })
}

// ── Public recording helpers ──────────────────────────────────────────────────

/// Record the latency of a completed `handle_event` call.
pub fn record_handle_latency(actor_type: &str, latency_ms: f64) {
    handle_latency_ms().record(
        latency_ms,
        &[KeyValue::new("actor_type", actor_type.to_string())],
    );
}

/// Increment the OCC-retry counter.
pub fn inc_occ_retry(actor_type: &str) {
    occ_retries().add(1, &[KeyValue::new("actor_type", actor_type.to_string())]);
}

/// Increment the spawn-call counter.
pub fn inc_spawn_call(actor_type: &str, capability: &str) {
    spawn_calls().add(
        1,
        &[
            KeyValue::new("actor_type", actor_type.to_string()),
            KeyValue::new("capability", capability.to_string()),
        ],
    );
}

/// Increment the spawn-error counter.
pub fn inc_spawn_error(actor_type: &str, capability: &str) {
    spawn_errors().add(
        1,
        &[
            KeyValue::new("actor_type", actor_type.to_string()),
            KeyValue::new("capability", capability.to_string()),
        ],
    );
}

/// Increment the handle-error counter.
pub fn inc_handle_error(actor_type: &str) {
    handle_errors().add(1, &[KeyValue::new("actor_type", actor_type.to_string())]);
}

/// Increment the handle-timeout counter.
pub fn inc_handle_timeout(actor_type: &str) {
    handle_timeouts().add(1, &[KeyValue::new("actor_type", actor_type.to_string())]);
}
