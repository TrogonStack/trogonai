//! Trait-based feature flag client used by [`AgentLoop`].
//!
//! [`AgentLoop`]: crate::agent_loop::AgentLoop

use std::future::Future;
use std::pin::Pin;

use trogon_splitio::flags::FeatureFlag;

/// Trait for checking whether a feature flag is enabled for a given tenant.
pub trait FeatureFlagClient: Send + Sync + 'static {
    fn is_enabled<'a>(
        &'a self,
        tenant_id: &'a str,
        flag: &'a dyn FeatureFlag,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>>;
}

/// Fail-open: all flags return `true` (used when Split.io is not configured).
pub struct AlwaysOnFlagClient;

impl FeatureFlagClient for AlwaysOnFlagClient {
    fn is_enabled<'a>(
        &'a self,
        _tenant_id: &'a str,
        _flag: &'a dyn FeatureFlag,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
}

/// Split.io-backed feature flag client.
pub struct SplitFlagClient {
    inner: trogon_splitio::SplitClient,
}

impl SplitFlagClient {
    pub fn new(inner: trogon_splitio::SplitClient) -> Self {
        Self { inner }
    }
}

impl FeatureFlagClient for SplitFlagClient {
    fn is_enabled<'a>(
        &'a self,
        tenant_id: &'a str,
        flag: &'a dyn FeatureFlag,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        let tenant = tenant_id.to_string();
        let flag_name = flag.name().to_string();
        let inner = self.inner.clone();
        Box::pin(async move {
            // Use get_treatment_or_control directly with the flag name string
            // to avoid object-safety issues with FeatureFlag.
            inner
                .get_treatment_or_control(&tenant, &flag_name, None)
                .await
                == "on"
        })
    }
}
