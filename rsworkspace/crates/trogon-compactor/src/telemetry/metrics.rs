//! Compaction billing/observability metrics (S4).
//!
//! Emits per-compaction OpenTelemetry instruments for cost attribution, using the
//! OTel **gen_ai semantic conventions** for attributes (`gen_ai.system`,
//! `gen_ai.request.model`, `gen_ai.response.model`, `gen_ai.token.type`) and the
//! standard `gen_ai.client.token.usage` token histogram. Compaction-specific
//! counters keep descriptive names (no standard convention exists for them) but
//! carry the same semantic-convention attributes. Lazily initialized against the
//! global meter provider installed by `trogon-telemetry`.

use std::sync::OnceLock;

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use opentelemetry_semantic_conventions::attribute::{
    GEN_AI_OPERATION_NAME, GEN_AI_REQUEST_MODEL, GEN_AI_RESPONSE_MODEL, GEN_AI_SYSTEM,
    GEN_AI_TOKEN_TYPE,
};
use opentelemetry_semantic_conventions::metric::GEN_AI_CLIENT_TOKEN_USAGE;

/// `gen_ai.operation.name` value for context compaction.
const OPERATION: &str = "compact";

static COMPACTIONS: OnceLock<Counter<u64>> = OnceLock::new();
static TOKEN_USAGE: OnceLock<Histogram<u64>> = OnceLock::new();
static TOKENS_SAVED: OnceLock<Histogram<u64>> = OnceLock::new();
static FALLBACKS: OnceLock<Counter<u64>> = OnceLock::new();

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogon-compactor")
}

fn compactions_counter() -> &'static Counter<u64> {
    COMPACTIONS.get_or_init(|| {
        meter()
            .u64_counter("trogon.compactor.compactions")
            .with_description("Compaction requests handled, by provider/model and whether compacted")
            .build()
    })
}

fn token_usage_histogram() -> &'static Histogram<u64> {
    TOKEN_USAGE.get_or_init(|| {
        meter()
            .u64_histogram(GEN_AI_CLIENT_TOKEN_USAGE)
            .with_description("Tokens seen by a compaction, by gen_ai.token.type (input/output)")
            .with_unit("{token}")
            .build()
    })
}

fn tokens_saved_histogram() -> &'static Histogram<u64> {
    TOKENS_SAVED.get_or_init(|| {
        meter()
            .u64_histogram("trogon.compactor.tokens_saved")
            .with_description("Tokens removed by a compaction (input - output)")
            .with_unit("{token}")
            .build()
    })
}

fn fallback_counter() -> &'static Counter<u64> {
    FALLBACKS.get_or_init(|| {
        meter()
            .u64_counter("trogon.compactor.fallbacks")
            .with_description("Compactions that fell back to the session model (M4)")
            .build()
    })
}

/// Record one handled compaction for billing/observability. `fallback_model` is
/// `Some` when M4 fell back to the session model.
pub fn record_compaction(
    provider: &str,
    model: &str,
    compacted: bool,
    tokens_before: u64,
    tokens_after: u64,
    fallback_model: Option<&str>,
) {
    let mut attrs = vec![
        KeyValue::new(GEN_AI_OPERATION_NAME, OPERATION),
        KeyValue::new(GEN_AI_SYSTEM, provider.to_string()),
        KeyValue::new(GEN_AI_REQUEST_MODEL, model.to_string()),
        KeyValue::new("compacted", compacted),
    ];
    if let Some(fallback) = fallback_model {
        attrs.push(KeyValue::new(GEN_AI_RESPONSE_MODEL, fallback.to_string()));
    }

    compactions_counter().add(1, &attrs);

    // Standard gen_ai token-usage histogram, split by token type.
    let mut input_attrs = attrs.clone();
    input_attrs.push(KeyValue::new(GEN_AI_TOKEN_TYPE, "input"));
    token_usage_histogram().record(tokens_before, &input_attrs);
    let mut output_attrs = attrs.clone();
    output_attrs.push(KeyValue::new(GEN_AI_TOKEN_TYPE, "output"));
    token_usage_histogram().record(tokens_after, &output_attrs);

    tokens_saved_histogram().record(tokens_before.saturating_sub(tokens_after), &attrs);
    if fallback_model.is_some() {
        fallback_counter().add(1, &attrs);
    }
}
