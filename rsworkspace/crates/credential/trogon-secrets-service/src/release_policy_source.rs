use trogon_credential_domain::{ConfiguredReleasePolicy, CredentialKey};

/// The release policy for each credential.
///
/// A credential with no policy releases to nobody: absence is never a wildcard.
pub trait ReleasePolicySource: Send + Sync + 'static {
    fn policy(&self, credential: &CredentialKey) -> Option<ConfiguredReleasePolicy>;
}
