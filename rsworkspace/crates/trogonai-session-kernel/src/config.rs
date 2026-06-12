use std::time::Duration;

use confique::Config;

/// Default inline artifact size limit (64 KiB).
pub const DEFAULT_INLINE_ARTIFACT_LIMIT_BYTES: usize = 65_536;
/// Default max event payload size (256 KiB).
pub const DEFAULT_MAX_EVENT_PAYLOAD_BYTES: usize = 262_144;
/// Default max snapshot size (5 MiB).
pub const DEFAULT_MAX_SNAPSHOT_BYTES: usize = 5_242_880;
/// Default session lease TTL.
pub const DEFAULT_LEASE_TTL_SECS: u64 = 30;
/// Default session lease renew interval.
pub const DEFAULT_LEASE_RENEW_INTERVAL_SECS: u64 = 10;
/// Default continuity checkpoint latency budget.
pub const DEFAULT_CHECKPOINT_LATENCY_BUDGET_SECS: u64 = 20;
/// Default switch latency budget without checkpoint.
pub const DEFAULT_SWITCH_LATENCY_BUDGET_SECS: u64 = 5;
/// Default NATS namespace prefix for session kernel buckets and streams.
pub const DEFAULT_NATS_PREFIX: &str = "ACP";

/// Session Kernel configuration loaded via ADR 0007 precedence:
/// built-in defaults → TOML → environment → CLI overrides.
#[derive(Config, Clone, Debug, PartialEq, Eq)]
pub struct SessionKernelConfig {
    #[config(env = "TROGON_SESSION_KERNEL_INLINE_ARTIFACT_LIMIT_BYTES", default = 65536)]
    pub inline_artifact_limit_bytes: usize,

    #[config(env = "TROGON_SESSION_KERNEL_MAX_EVENT_PAYLOAD_BYTES", default = 262144)]
    pub max_event_payload_bytes: usize,

    #[config(env = "TROGON_SESSION_KERNEL_MAX_SNAPSHOT_BYTES", default = 5242880)]
    pub max_snapshot_bytes: usize,

    #[config(env = "TROGON_SESSION_KERNEL_LEASE_TTL_SECS", default = 30)]
    lease_ttl_secs: u64,

    #[config(env = "TROGON_SESSION_KERNEL_LEASE_RENEW_INTERVAL_SECS", default = 10)]
    lease_renew_interval_secs: u64,

    #[config(
        env = "TROGON_SESSION_KERNEL_CHECKPOINT_LATENCY_BUDGET_SECS",
        default = 20
    )]
    checkpoint_latency_budget_secs: u64,

    #[config(env = "TROGON_SESSION_KERNEL_SWITCH_LATENCY_BUDGET_SECS", default = 5)]
    switch_latency_budget_secs: u64,

    #[config(env = "TROGON_SESSION_KERNEL_NATS_PREFIX", default = "ACP")]
    pub nats_prefix: String,
}

impl SessionKernelConfig {
    pub fn lease_ttl(&self) -> Duration {
        Duration::from_secs(self.lease_ttl_secs)
    }

    pub fn lease_renew_interval(&self) -> Duration {
        Duration::from_secs(self.lease_renew_interval_secs)
    }

    pub fn checkpoint_latency_budget(&self) -> Duration {
        Duration::from_secs(self.checkpoint_latency_budget_secs)
    }

    pub fn switch_latency_budget(&self) -> Duration {
        Duration::from_secs(self.switch_latency_budget_secs)
    }
}

impl Default for SessionKernelConfig {
    fn default() -> Self {
        Self::builder().load().expect("session kernel config defaults")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documented_values() {
        let config = SessionKernelConfig::default();
        assert_eq!(
            config.inline_artifact_limit_bytes,
            DEFAULT_INLINE_ARTIFACT_LIMIT_BYTES
        );
        assert_eq!(
            config.max_event_payload_bytes,
            DEFAULT_MAX_EVENT_PAYLOAD_BYTES
        );
        assert_eq!(config.max_snapshot_bytes, DEFAULT_MAX_SNAPSHOT_BYTES);
        assert_eq!(config.lease_ttl(), Duration::from_secs(30));
        assert_eq!(config.lease_renew_interval(), Duration::from_secs(10));
        assert_eq!(config.checkpoint_latency_budget(), Duration::from_secs(20));
        assert_eq!(config.switch_latency_budget(), Duration::from_secs(5));
        assert_eq!(config.nats_prefix, DEFAULT_NATS_PREFIX);
    }
}
