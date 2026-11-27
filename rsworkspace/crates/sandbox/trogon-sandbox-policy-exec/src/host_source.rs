use crate::constants::HOST_SOURCE_TTL_SECONDS;
use trogon_sandbox_contract::SecretName;
use trogon_sandbox_contract::{CredentialProvider, CredentialRequest, CredentialScope, PullSecretId};

/// The request a binding carries when its value is read from the host. Only
/// the pull secret id is read back, as the name of the binding.
pub fn host_source_request(name: &SecretName) -> CredentialRequest {
    CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new(name.as_str()),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: HOST_SOURCE_TTL_SECONDS,
        requester: None,
    }
}
