use std::collections::BTreeMap;
use std::path::Path;

use trogon_credential_domain::{ConfiguredReleasePolicy, CredentialKey};
use trogon_std::ReadFile;

use super::release_policies_file_wire::ReleasePoliciesFileWire;
use super::{ReleasePoliciesFileError, ReleasePolicySource, TomlSyntax};

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

    /// Reads the release policies file at `path`, once, at startup.
    ///
    /// Each entry names one credential by its owner and id, the address a
    /// release request carries, with every dimension of its release:
    ///
    /// ```toml
    /// [[policies]]
    /// owner_id = "project-a"
    /// credential_id = "0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f"
    /// runtime_services = ["billing-runner"]
    /// hosts = ["api.github.com"]
    /// headers = ["authorization"]
    /// query_parameters = []
    /// body_fields = []
    /// cache_ttl_secs = 120
    /// ```
    ///
    /// There is no way to write an unrestricted dimension: runtime services
    /// and hosts must be listed, at least one injection location must be, and
    /// `cache_ttl_secs` may only shorten [`RELEASE_TTL_BOUND`]. A credential
    /// the file does not name has no policy, which denies its release.
    ///
    /// [`RELEASE_TTL_BOUND`]: crate::RELEASE_TTL_BOUND
    ///
    /// # Errors
    ///
    /// Returns [`ReleasePoliciesFileError`] when the file cannot be read or
    /// parsed, names no policies, holds an entry that is not a complete
    /// policy, or gives one credential two policies.
    pub fn load(path: &Path, fs: &impl ReadFile) -> Result<Self, ReleasePoliciesFileError> {
        let document = fs
            .read_to_string(path)
            .map_err(|source| ReleasePoliciesFileError::Unreadable {
                path: path.to_owned(),
                source,
            })?;
        let wire: ReleasePoliciesFileWire =
            toml::from_str(&document).map_err(|error| ReleasePoliciesFileError::Unparsable {
                path: path.to_owned(),
                syntax: TomlSyntax::of(&error, &document),
            })?;
        if wire.policies.is_empty() {
            return Err(ReleasePoliciesFileError::NoPolicies { path: path.to_owned() });
        }
        let mut entries = BTreeMap::<CredentialKey, usize>::new();
        let mut loaded = Self::new();
        for (entry, written) in (1..).zip(wire.policies) {
            let (credential, policy) =
                written
                    .into_policy()
                    .map_err(|source| ReleasePoliciesFileError::InvalidPolicy {
                        path: path.to_owned(),
                        entry,
                        source,
                    })?;
            if let Some(first) = entries.insert(credential.clone(), entry) {
                return Err(ReleasePoliciesFileError::DuplicatePolicy {
                    path: path.to_owned(),
                    credential,
                    first,
                    second: entry,
                });
            }
            loaded = loaded.with_policy(credential, policy);
        }
        Ok(loaded)
    }
}

impl ReleasePolicySource for StaticReleasePolicies {
    fn policy(&self, credential: &CredentialKey) -> Option<ConfiguredReleasePolicy> {
        self.policies.get(credential).cloned()
    }
}

#[cfg(test)]
mod tests;
