use std::collections::BTreeMap;

use trogon_credential_domain::{ConfiguredReleasePolicy, CredentialKey};

use super::ReleasePolicySource;

/// Release policies loaded once at startup, keyed by credential.
#[derive(Clone, Debug, Default)]
pub struct StaticReleasePolicies {
    policies: BTreeMap<CredentialKey, ConfiguredReleasePolicy>,
}

impl StaticReleasePolicies {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the policy for `credential`, replacing any earlier one.
    #[must_use]
    pub fn with_policy(mut self, credential: CredentialKey, policy: ConfiguredReleasePolicy) -> Self {
        self.policies.insert(credential, policy);
        self
    }
}

impl ReleasePolicySource for StaticReleasePolicies {
    fn policy(&self, credential: &CredentialKey) -> Option<ConfiguredReleasePolicy> {
        self.policies.get(credential).cloned()
    }
}

#[cfg(test)]
mod tests;
