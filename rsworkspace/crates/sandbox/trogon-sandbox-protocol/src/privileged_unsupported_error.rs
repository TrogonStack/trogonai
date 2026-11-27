use trogon_sandbox_contract::ProviderKind;

/// A lease asked for elevated capabilities on a provider that cannot grant them.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("privileged: true is not supported on the {provider:?} provider")]
pub struct PrivilegedUnsupportedError {
    provider: ProviderKind,
}

impl PrivilegedUnsupportedError {
    pub(crate) fn new(provider: ProviderKind) -> Self {
        Self { provider }
    }

    /// The provider that refused.
    pub fn provider(&self) -> ProviderKind {
        self.provider
    }
}
