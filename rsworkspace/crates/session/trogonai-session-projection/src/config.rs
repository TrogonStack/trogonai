use confique::Config;

/// Default reserved token budget for model output.
pub const DEFAULT_OUTPUT_RESERVE_TOKENS: u64 = 4_096;
/// Default number of recent turns to prefer when projecting.
pub const DEFAULT_RECENT_TURN_COUNT: usize = 8;
/// Default NATS namespace prefix for projection KV buckets.
pub const DEFAULT_NATS_PREFIX: &str = "ACP";

/// Prompt projection configuration loaded via ADR 0007 precedence.
#[derive(Config, Clone, Debug, PartialEq, Eq)]
pub struct ProjectionConfig {
    #[config(env = "TROGON_SESSION_PROJECTION_OUTPUT_RESERVE_TOKENS", default = 4096)]
    pub output_reserve_tokens: u64,

    #[config(env = "TROGON_SESSION_PROJECTION_RECENT_TURN_COUNT", default = 8)]
    pub recent_turn_count: usize,

    #[config(env = "TROGON_SESSION_PROJECTION_NATS_PREFIX", default = "ACP")]
    pub nats_prefix: String,

    #[config(env = "TROGON_SESSION_PROJECTION_MAX_CONTEXT_TWIN_BYTES", default = 262144)]
    pub max_context_twin_bytes: usize,
}

impl Default for ProjectionConfig {
    fn default() -> Self {
        Self::builder().load().expect("projection config defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_values() {
        let config = ProjectionConfig::default();
        assert_eq!(config.output_reserve_tokens, DEFAULT_OUTPUT_RESERVE_TOKENS);
        assert_eq!(config.recent_turn_count, DEFAULT_RECENT_TURN_COUNT);
        assert_eq!(config.nats_prefix, DEFAULT_NATS_PREFIX);
    }
}
