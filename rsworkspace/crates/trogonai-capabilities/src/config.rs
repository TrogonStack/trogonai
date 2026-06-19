use std::time::Duration;

use confique::Config;

/// Configuration for capability registry freshness and conservative fallbacks.
///
/// Per ADR-0007 / § "Feature flags, limites, TTLs, retention, SLOs y rollout deben tener
/// un nombre canonico tipado en config Rust": these are canonical typed config values with
/// env overrides, so freshness TTL and conservative fallbacks are tunable with real data
/// (§ "Ajustar limites y SLOs con datos reales") without recompiling.
#[derive(Config, Debug, Clone, PartialEq)]
pub struct CapabilityConfig {
    /// Default TTL (seconds) applied when a schema omits `ttl_seconds`.
    #[config(env = "TROGON_CAPABILITY_DEFAULT_TTL_SECS", default = 86400)]
    pub default_ttl_secs: u64,
    /// Minimum confidence required to trust a non-registry capability source.
    #[config(env = "TROGON_CAPABILITY_MIN_CONFIDENCE", default = 0.5)]
    pub min_confidence: f64,
    /// Conservative context window when capabilities are stale or missing.
    #[config(env = "TROGON_CAPABILITY_CONSERVATIVE_MAX_CONTEXT_TOKENS", default = 8192)]
    pub conservative_max_context_tokens: u64,
    /// Conservative output limit when capabilities are stale or missing.
    #[config(env = "TROGON_CAPABILITY_CONSERVATIVE_MAX_OUTPUT_TOKENS", default = 2048)]
    pub conservative_max_output_tokens: u64,
}

impl CapabilityConfig {
    /// Default TTL applied when a schema omits `ttl_seconds`.
    pub fn default_ttl(&self) -> Duration {
        Duration::from_secs(self.default_ttl_secs)
    }
}

impl Default for CapabilityConfig {
    fn default() -> Self {
        Self::builder().load().expect("capability config defaults")
    }
}
