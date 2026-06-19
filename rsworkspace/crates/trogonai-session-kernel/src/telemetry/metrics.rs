use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use std::sync::OnceLock;

static EVENT_APPEND: OnceLock<Counter<u64>> = OnceLock::new();
static EVENT_DEDUP: OnceLock<Counter<u64>> = OnceLock::new();
static LEASE_CONTENTION: OnceLock<Counter<u64>> = OnceLock::new();
static SNAPSHOT_STALE: OnceLock<Counter<u64>> = OnceLock::new();
static SWITCH_LATENCY_MS: OnceLock<Histogram<f64>> = OnceLock::new();
static SHADOW_SYNC: OnceLock<Counter<u64>> = OnceLock::new();
static SHADOW_DIVERGENCE: OnceLock<Counter<u64>> = OnceLock::new();
static MIGRATION_ON_DEMAND: OnceLock<Counter<u64>> = OnceLock::new();
static ROLLOUT_ENFORCEMENT: OnceLock<Counter<u64>> = OnceLock::new();

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogonai-session-kernel")
}

fn event_append_counter() -> &'static Counter<u64> {
    EVENT_APPEND.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.event.append")
            .with_description("Session events appended to JetStream")
            .build()
    })
}

fn event_dedup_counter() -> &'static Counter<u64> {
    EVENT_DEDUP.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.event.dedup")
            .with_description("Session event append deduplicated by idempotency key")
            .build()
    })
}

fn lease_contention_counter() -> &'static Counter<u64> {
    LEASE_CONTENTION.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.lease.contention")
            .with_description("Session lease acquire rejected because session is busy")
            .build()
    })
}

fn snapshot_stale_counter() -> &'static Counter<u64> {
    SNAPSHOT_STALE.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.snapshot.stale")
            .with_description("Materialized snapshot lagged behind event log during recovery")
            .build()
    })
}

fn switch_latency_histogram() -> &'static Histogram<f64> {
    SWITCH_LATENCY_MS.get_or_init(|| {
        meter()
            .f64_histogram("trogonai.session_kernel.switch.latency_ms")
            .with_description("Model switch operation latency in milliseconds")
            .with_unit("ms")
            .build()
    })
}

fn shadow_sync_counter() -> &'static Counter<u64> {
    SHADOW_SYNC.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.shadow.sync")
            .with_description("Shadow-mode legacy export synced to canonical snapshot")
            .build()
    })
}

fn shadow_divergence_counter() -> &'static Counter<u64> {
    SHADOW_DIVERGENCE.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.shadow.divergence")
            .with_description("Shadow snapshot diverged from legacy export")
            .build()
    })
}

fn migration_on_demand_counter() -> &'static Counter<u64> {
    MIGRATION_ON_DEMAND.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.migration.on_demand")
            .with_description("Legacy sessions migrated on open/modify")
            .build()
    })
}

pub fn record_event_appended(session_id: &str, event_type: &str) {
    event_append_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("event_type", event_type.to_string()),
        ],
    );
}

pub fn record_event_deduplicated(session_id: &str) {
    event_dedup_counter().add(1, &[KeyValue::new("session_id", session_id.to_string())]);
}

pub fn record_lease_contention(session_id: &str) {
    lease_contention_counter().add(1, &[KeyValue::new("session_id", session_id.to_string())]);
}

pub fn record_snapshot_stale(session_id: &str) {
    snapshot_stale_counter().add(1, &[KeyValue::new("session_id", session_id.to_string())]);
}

#[allow(clippy::too_many_arguments)]
pub fn record_switch_latency_ms(
    session_id: &str,
    operation_id: &str,
    latency_ms: f64,
    switch_result: &str,
    source_runner: &str,
    target_runner: &str,
    source_model: &str,
    target_model: &str,
) {
    switch_latency_histogram().record(
        latency_ms,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("switch_result", switch_result.to_string()),
            KeyValue::new("source_runner", source_runner.to_string()),
            KeyValue::new("target_runner", target_runner.to_string()),
            KeyValue::new("source_model", source_model.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
        ],
    );
}

pub fn record_shadow_sync(session_id: &str, message_count: usize) {
    shadow_sync_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("message_count", message_count.to_string()),
        ],
    );
}

pub fn record_shadow_divergence(session_id: &str, mismatched_roles: usize) {
    shadow_divergence_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("mismatched_roles", mismatched_roles.to_string()),
        ],
    );
}

fn rollout_enforcement_counter() -> &'static Counter<u64> {
    ROLLOUT_ENFORCEMENT.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_kernel.rollout.enforcement")
            .with_description("Rollout promotion/rollback enforcement decision based on measured metrics")
            .build()
    })
}

/// Record a rollout enforcement decision (§ "enforcement de promocion/rollback basada en
/// metricas"). `decision` is `promote` or `rollback`; `blocked_reasons` is the number of
/// breached thresholds (0 on promote).
pub fn record_rollout_enforcement(decision: &str, blocked_reasons: usize) {
    rollout_enforcement_counter().add(
        1,
        &[
            KeyValue::new("decision", decision.to_string()),
            KeyValue::new("blocked_reasons", blocked_reasons.to_string()),
        ],
    );
}

pub fn record_on_demand_migration(session_id: &str, legacy_message_count: usize) {
    migration_on_demand_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("legacy_message_count", legacy_message_count.to_string()),
        ],
    );
}
