use std::future::Future;

use trogon_credential_domain::CredentialKey;
use trogonai_proto::gateway::credentials::state_v1::CredentialStateSnapshot;

use super::CredentialStateReadError;

/// The current state of one credential, folded from its stream.
pub trait CredentialStateRead: Send + Sync + 'static {
    /// Folds the credential's stream. `None` means the stream holds no events
    /// for it.
    fn read(
        &self,
        credential: &CredentialKey,
    ) -> impl Future<Output = Result<Option<CredentialStateSnapshot>, CredentialStateReadError>> + Send;
}
