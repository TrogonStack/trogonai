//! Compaction billing/observability metrics (S4).
//!
//! Emits per-compaction OpenTelemetry instruments for cost attribution, using the
//! OTel **gen_ai semantic conventions** for the *used* `(provider, model)` pair
//! (`gen_ai.system`, `gen_ai.request.model`, `gen_ai.response.model`,
//! `gen_ai.token.type`) and the standard `gen_ai.client.token.usage` token
//! histogram, so token histograms stay queryable by the conventional keys. In
//! addition, every instrument carries the four explicit S4 identity attributes
//! (`session_provider`, `session_model`, `compactor_provider`, `compactor_model`)
//! plus `fallback_model`/`fallback_reason`/`error_kind` where applicable.
//! Compaction-specific counters/histograms keep descriptive names (no standard
//! convention exists for them). Lazily initialized against the global meter
//! provider installed by `trogon-telemetry`.

use std::sync::OnceLock;

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};

// `opentelemetry-semantic-conventions` deprecated its `gen_ai.*` re-exports in
// favor of the standalone GenAI semantic conventions repository, but the
// attribute/metric key strings themselves are unchanged. Inline them here to
// keep emitting the conventional keys without depending on the deprecated
// constants (the workspace denies `deprecated` as a warning).
const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";
const GEN_AI_SYSTEM: &str = "gen_ai.system";
const GEN_AI_TOKEN_TYPE: &str = "gen_ai.token.type";
const GEN_AI_CLIENT_TOKEN_USAGE: &str = "gen_ai.client.token.usage";

/// `gen_ai.operation.name` value for context compaction.
const OPERATION: &str = "compact";

static REQUESTS: OnceLock<Counter<u64>> = OnceLock::new();
static SUCCESSES: OnceLock<Counter<u64>> = OnceLock::new();
static TOKEN_USAGE: OnceLock<Histogram<u64>> = OnceLock::new();
static TOKENS_SAVED: OnceLock<Histogram<u64>> = OnceLock::new();
static FALLBACKS: OnceLock<Counter<u64>> = OnceLock::new();
static DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
static COMPRESSION_RATIO: OnceLock<Histogram<u64>> = OnceLock::new();
static FAILURES: OnceLock<Counter<u64>> = OnceLock::new();

/// `error_kind` attribute key for the failure counter.
const ERROR_KIND_KEY: &str = "error_kind";
/// Explicit S4 identity attribute keys (in addition to the `gen_ai.*` conventions
/// carried for the *used* pair). Low cardinality: bounded by configured providers/models.
const SESSION_PROVIDER_KEY: &str = "session_provider";
const SESSION_MODEL_KEY: &str = "session_model";
const COMPACTOR_PROVIDER_KEY: &str = "compactor_provider";
const COMPACTOR_MODEL_KEY: &str = "compactor_model";
const FALLBACK_MODEL_KEY: &str = "fallback_model";
const FALLBACK_REASON_KEY: &str = "fallback_reason";

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogon-compactor")
}

fn requests_counter() -> &'static Counter<u64> {
    REQUESTS.get_or_init(|| {
        meter()
            .u64_counter("trogon.compactor.requests_total")
            .with_description("Every compaction request that entered service::handle (all attempts)")
            .build()
    })
}

fn success_counter() -> &'static Counter<u64> {
    SUCCESSES.get_or_init(|| {
        meter()
            .u64_counter("trogon.compactor.success_total")
            .with_description("Compaction requests that produced a response (incl. via M4 fallback)")
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
            .u64_counter("trogon.compactor.fallback_total")
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
            .u64_counter("trogon.compactor.failure_total")
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

/// Increment `trogon.compactor.requests_total`. Called once on entry to
/// `service::handle`, before any compaction attempt, for every request.
pub fn record_request() {
    requests_counter().add(1, &[KeyValue::new(GEN_AI_OPERATION_NAME, OPERATION)]);
}

/// The four explicit S4 identity attributes carried by every per-compaction
/// instrument: the session `(provider, model)` and the compactor `(provider, model)`.
/// On the M4 fallback path the *used* pair is the session pair, while
/// `compactor_*` still reflects the originally-chosen compactor identity.
#[derive(Clone, Copy)]
pub struct CompactionIdentity<'a> {
    pub session_provider: &'a str,
    pub session_model: &'a str,
    pub compactor_provider: &'a str,
    pub compactor_model: &'a str,
}

impl CompactionIdentity<'_> {
    fn identity_attrs(&self) -> [KeyValue; 4] {
        [
            KeyValue::new(SESSION_PROVIDER_KEY, self.session_provider.to_string()),
            KeyValue::new(SESSION_MODEL_KEY, self.session_model.to_string()),
            KeyValue::new(COMPACTOR_PROVIDER_KEY, self.compactor_provider.to_string()),
            KeyValue::new(COMPACTOR_MODEL_KEY, self.compactor_model.to_string()),
        ]
    }
}

/// Record one handled compaction (success path, incl. M4 fallback) for
/// billing/observability.
///
/// `used_provider`/`used_model` are the pair that actually produced the summary;
/// they are tagged with the `gen_ai.*` conventions so the token histograms stay
/// queryable by convention. `identity` carries the four explicit S4 attributes.
/// `fallback` is `Some((fallback_model, fallback_reason))` when M4 fell back to the
/// session model — in that case the *used* pair equals the session pair and the
/// reason is a stable, low-cardinality classification of the primary error.
/// `duration_ms` is the wall-clock latency of the compaction call.
#[allow(clippy::too_many_arguments)]
pub fn record_compaction(
    used_provider: &str,
    used_model: &str,
    identity: CompactionIdentity<'_>,
    compacted: bool,
    tokens_before: u64,
    tokens_after: u64,
    duration_ms: u64,
    fallback: Option<(&str, &str)>,
) {
    let mut attrs = vec![
        KeyValue::new(GEN_AI_OPERATION_NAME, OPERATION),
        KeyValue::new(GEN_AI_SYSTEM, used_provider.to_string()),
        KeyValue::new(GEN_AI_REQUEST_MODEL, used_model.to_string()),
        KeyValue::new("compacted", compacted),
    ];
    attrs.extend(identity.identity_attrs());
    if let Some((fallback_model, fallback_reason)) = fallback {
        attrs.push(KeyValue::new(GEN_AI_RESPONSE_MODEL, fallback_model.to_string()));
        attrs.push(KeyValue::new(FALLBACK_MODEL_KEY, fallback_model.to_string()));
        attrs.push(KeyValue::new(FALLBACK_REASON_KEY, fallback_reason.to_string()));
    }

    success_counter().add(1, &attrs);

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
    if fallback.is_some() {
        fallback_counter().add(1, &attrs);
    }
}

/// Record a compaction failure. `error_kind` is a stable, low-cardinality
/// classification of the error (the enum variant name, not its `Display`).
/// `used_provider`/`used_model` are the pair that failed; `identity` carries the
/// four explicit S4 identity attributes.
pub fn record_failure(
    used_provider: &str,
    used_model: &str,
    identity: CompactionIdentity<'_>,
    error_kind: &str,
) {
    let mut attrs = vec![
        KeyValue::new(GEN_AI_OPERATION_NAME, OPERATION),
        KeyValue::new(GEN_AI_SYSTEM, used_provider.to_string()),
        KeyValue::new(GEN_AI_REQUEST_MODEL, used_model.to_string()),
        KeyValue::new(ERROR_KIND_KEY, error_kind.to_string()),
    ];
    attrs.extend(identity.identity_attrs());
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
