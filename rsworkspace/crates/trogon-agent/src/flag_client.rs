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

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_splitio::{SplitClient, SplitConfig, mock::MockEvaluator};

    async fn client_for_mock(mock: MockEvaluator) -> SplitFlagClient {
        let (addr, _handle) = mock.serve().await;
        SplitFlagClient::new(SplitClient::new(SplitConfig {
            evaluator_url: format!("http://{addr}"),
            auth_token: "any".to_string(),
        }))
    }

    struct TestFlag(&'static str);
    impl FeatureFlag for TestFlag {
        fn name(&self) -> &'static str {
            self.0
        }
    }

    /// Treatment "on" (exact, lowercase) → `is_enabled` returns true.
    #[tokio::test]
    async fn split_flag_client_on_treatment_returns_true() {
        let client = client_for_mock(MockEvaluator::new().with_flag("my_flag", "on")).await;
        let enabled = client.is_enabled("tenant-1", &TestFlag("my_flag")).await;
        assert!(enabled, "treatment 'on' must return true");
    }

    /// Treatment "off" → `is_enabled` returns false.
    #[tokio::test]
    async fn split_flag_client_off_treatment_returns_false() {
        let client = client_for_mock(MockEvaluator::new().with_flag("my_flag", "off")).await;
        let enabled = client.is_enabled("tenant-1", &TestFlag("my_flag")).await;
        assert!(!enabled, "treatment 'off' must return false");
    }

    /// Treatment "control" (default for undefined flags) → returns false.
    #[tokio::test]
    async fn split_flag_client_control_treatment_returns_false() {
        // Flag not defined in mock → evaluator returns "control".
        let client = client_for_mock(MockEvaluator::new()).await;
        let enabled = client.is_enabled("tenant-1", &TestFlag("undefined_flag")).await;
        assert!(!enabled, "treatment 'control' must return false");
    }

    /// The comparison is case-sensitive: "ON" is NOT "on" → returns false.
    /// Split.io SDK always returns lowercase, but this documents the contract.
    #[tokio::test]
    async fn split_flag_client_on_uppercase_is_not_enabled() {
        let client = client_for_mock(MockEvaluator::new().with_flag("my_flag", "ON")).await;
        let enabled = client.is_enabled("tenant-1", &TestFlag("my_flag")).await;
        assert!(!enabled, "treatment 'ON' (uppercase) must return false — comparison is case-sensitive");
    }

    /// `AlwaysOnFlagClient` always returns true regardless of flag or tenant.
    #[tokio::test]
    async fn always_on_flag_client_returns_true() {
        let client = AlwaysOnFlagClient;
        assert!(client.is_enabled("any-tenant", &TestFlag("any_flag")).await);
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
