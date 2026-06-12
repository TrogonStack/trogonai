use std::sync::OnceLock;

use opentelemetry::metrics::Counter;
use opentelemetry::{KeyValue, global};

static CAPABILITY_RESOLVED: OnceLock<Counter<u64>> = OnceLock::new();
static CAPABILITY_DEGRADATION: OnceLock<Counter<u64>> = OnceLock::new();
static ADAPTATION_PLAN_CREATED: OnceLock<Counter<u64>> = OnceLock::new();

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogonai-capabilities")
}

fn capability_resolved_counter() -> &'static Counter<u64> {
    CAPABILITY_RESOLVED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.capabilities.resolved")
            .with_description("Capability schemas resolved from AGENT_REGISTRY")
            .build()
    })
}

fn capability_degradation_counter() -> &'static Counter<u64> {
    CAPABILITY_DEGRADATION.get_or_init(|| {
        meter()
            .u64_counter("trogonai.capabilities.degradation")
            .with_description("Capability degradations recorded during switch negotiation")
            .build()
    })
}

fn adaptation_plan_created_counter() -> &'static Counter<u64> {
    ADAPTATION_PLAN_CREATED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.capabilities.adaptation_plan.created")
            .with_description("Switch adaptation plans created")
            .build()
    })
}

pub fn record_capability_resolved(
    model_id: &str,
    runner_id: &str,
    freshness: &str,
    degraded: bool,
    confidence: f64,
) {
    capability_resolved_counter().add(
        1,
        &[
            KeyValue::new("source_model", model_id.to_string()),
            KeyValue::new("source_runner", runner_id.to_string()),
            KeyValue::new("capability_freshness", freshness.to_string()),
            KeyValue::new("capability_degraded", degraded.to_string()),
            KeyValue::new("confidence", confidence.to_string()),
        ],
    );
}

pub fn record_capability_degradation(session_id: &str, capability: &str, action: &str) {
    capability_degradation_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("capability", capability.to_string()),
            KeyValue::new("capability_degradation", action.to_string()),
        ],
    );
}

pub fn record_adaptation_plan_created(session_id: &str, from_model: &str, to_model: &str, adaptation_count: usize) {
    adaptation_plan_created_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("source_model", from_model.to_string()),
            KeyValue::new("target_model", to_model.to_string()),
            KeyValue::new("adaptation_count", adaptation_count.to_string()),
        ],
    );
}
