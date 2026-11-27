use crate::constants::DEFAULT_ACTIVE_LEASE_BUCKET;

/// Where the observability reads live in the broker's KV state.
#[derive(Clone)]
pub struct ObservabilityConfig {
    pub active_lease_bucket: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            active_lease_bucket: DEFAULT_ACTIVE_LEASE_BUCKET.to_string(),
        }
    }
}
