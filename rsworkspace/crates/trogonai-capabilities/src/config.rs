use std::time::Duration;

/// Configuration for capability registry freshness and conservative fallbacks.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityConfig {
    /// Default TTL applied when a schema omits `ttl_seconds`.
    pub default_ttl: Duration,
    /// Minimum confidence required to trust a non-registry capability source.
    pub min_confidence: f64,
    /// Conservative context window when capabilities are stale or missing.
    pub conservative_max_context_tokens: u64,
    /// Conservative output limit when capabilities are stale or missing.
    pub conservative_max_output_tokens: u64,
}

impl Default for CapabilityConfig {
    fn default() -> Self {
        Self {
            default_ttl: Duration::from_secs(86_400),
            min_confidence: 0.5,
            conservative_max_context_tokens: 8_192,
            conservative_max_output_tokens: 2_048,
        }
    }
}
