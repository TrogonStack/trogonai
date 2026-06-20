use std::sync::OnceLock;

use opentelemetry::metrics::Counter;
use opentelemetry::{KeyValue, global};

static CONTEXT_TWIN_UPDATED: OnceLock<Counter<u64>> = OnceLock::new();
static PROMPT_COMPILED: OnceLock<Counter<u64>> = OnceLock::new();
static PROJECTION_DEGRADED: OnceLock<Counter<u64>> = OnceLock::new();

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogonai-session-projection")
}

fn context_twin_updated_counter() -> &'static Counter<u64> {
    CONTEXT_TWIN_UPDATED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_projection.context_twin_updated")
            .with_description("Context Twin snapshots derived and persisted")
            .build()
    })
}

fn prompt_compiled_counter() -> &'static Counter<u64> {
    PROMPT_COMPILED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_projection.prompt_compiled")
            .with_description("Prompt projections compiled for a target model")
            .build()
    })
}

fn projection_degraded_counter() -> &'static Counter<u64> {
    PROJECTION_DEGRADED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.session_projection.degraded")
            .with_description("Prompt projection excluded blocks due to token budget")
            .build()
    })
}

pub fn record_context_twin_updated(session_id: &str, derived_from_seq: u64) {
    context_twin_updated_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("derived_from_seq", derived_from_seq.to_string()),
        ],
    );
}

pub fn record_prompt_compiled(session_id: &str, model_id: &str, token_estimate: u64) {
    prompt_compiled_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("model_id", model_id.to_string()),
            KeyValue::new("token_estimate", token_estimate.to_string()),
        ],
    );
}

pub fn record_projection_degraded(session_id: &str, model_id: &str, excluded_blocks: usize) {
    projection_degraded_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("model_id", model_id.to_string()),
            KeyValue::new("excluded_blocks", excluded_blocks.to_string()),
        ],
    );
}
