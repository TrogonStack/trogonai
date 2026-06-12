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
static DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
static COMPRESSION_RATIO: OnceLock<Histogram<u64>> = OnceLock::new();
static FAILURES: OnceLock<Counter<u64>> = OnceLock::new();

/// `error.type` attribute key (OTel semantic convention) for the failure counter.
const ERROR_KIND_KEY: &str = "error_kind";

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

fn duration_histogram() -> &'static Histogram<u64> {
    DURATION.get_or_init(|| {
        meter()
            .u64_histogram("trogon.compactor.duration")
            .with_description("Wall-clock latency of a handled compaction, in milliseconds")
            .with_unit("ms")
            .build()
    })
}

fn compression_ratio_histogram() -> &'static Histogram<u64> {
    COMPRESSION_RATIO.get_or_init(|| {
        meter()
            .u64_histogram("trogon.compactor.compression_ratio")
            // Recorded as an integer percentage in [0, 100]:
            // tokens_after * 100 / tokens_before (saturated to 0 when tokens_before == 0).
            // Lower is better (more compression).
            .with_description("Post/pre compaction token ratio, as an integer percentage (0..=100)")
            .with_unit("%")
            .build()
    })
}

fn failure_counter() -> &'static Counter<u64> {
    FAILURES.get_or_init(|| {
        meter()
            .u64_counter("trogon.compactor.failures")
            .with_description("Compactions that failed, by provider/model and error_kind")
            .build()
    })
}

/// Compression ratio recorded by the histogram: `tokens_after * 100 / tokens_before`
/// as an integer percentage in `[0, 100]`. Saturates to `0` when `tokens_before == 0`
/// (no input to compress). Pure helper, extracted for testability.
pub(crate) fn compression_ratio_pct(tokens_before: u64, tokens_after: u64) -> u64 {
    if tokens_before == 0 {
        return 0;
    }
    // tokens_after should not exceed tokens_before in practice; clamp defensively.
    let ratio = tokens_after.saturating_mul(100) / tokens_before;
    ratio.min(100)
}

/// Record one handled compaction for billing/observability. `fallback_model` is
/// `Some` when M4 fell back to the session model. `duration_ms` is the wall-clock
/// latency of the compaction call.
pub fn record_compaction(
    provider: &str,
    model: &str,
    compacted: bool,
    tokens_before: u64,
    tokens_after: u64,
    duration_ms: u64,
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
    duration_histogram().record(duration_ms, &attrs);
    compression_ratio_histogram().record(compression_ratio_pct(tokens_before, tokens_after), &attrs);
    if fallback_model.is_some() {
        fallback_counter().add(1, &attrs);
    }
}

/// Record a compaction failure. `error_kind` is a stable, low-cardinality
/// classification of the error (the enum variant name, not its `Display`).
pub fn record_failure(provider: &str, model: &str, error_kind: &str) {
    let attrs = [
        KeyValue::new(GEN_AI_OPERATION_NAME, OPERATION),
        KeyValue::new(GEN_AI_SYSTEM, provider.to_string()),
        KeyValue::new(GEN_AI_REQUEST_MODEL, model.to_string()),
        KeyValue::new(ERROR_KIND_KEY, error_kind.to_string()),
    ];
    failure_counter().add(1, &attrs);
}

#[cfg(test)]
mod tests {
    use super::compression_ratio_pct;

    #[test]
    fn compression_ratio_zero_before_saturates_to_zero() {
        assert_eq!(compression_ratio_pct(0, 0), 0);
        assert_eq!(compression_ratio_pct(0, 42), 0);
    }

    #[test]
    fn compression_ratio_is_integer_percentage() {
        // Half the tokens remain -> 50%.
        assert_eq!(compression_ratio_pct(1000, 500), 50);
        // No compression -> 100%.
        assert_eq!(compression_ratio_pct(1000, 1000), 100);
        // Everything removed -> 0%.
        assert_eq!(compression_ratio_pct(1000, 0), 0);
        // Integer division truncates toward zero (333/1000 -> 33%).
        assert_eq!(compression_ratio_pct(1000, 333), 33);
    }

    #[test]
    fn compression_ratio_clamps_above_100() {
        // Defensive: tokens_after > tokens_before clamps to 100, never overflows.
        assert_eq!(compression_ratio_pct(100, 200), 100);
        assert_eq!(compression_ratio_pct(1, u64::MAX), 100);
    }
}
