use std::num::NonZeroU32;

use tracing::warn;
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, LeaseId, PullSecretId, ServiceAccountId, TenantId};
use trogon_sandbox_credential_client::{ClientError, CredentialClient, IssueParams};

use crate::{SessionCredential, SshSessionId};

/// Registers each gateway session with the credential broker so its token can be revoked when the session ends.
pub struct SessionKeyMint {
    client: CredentialClient<async_nats::Client>,
    pull_secret_id: PullSecretId,
}

impl SessionKeyMint {
    pub fn new(client: async_nats::Client, tenant: TenantId, pull_secret_id: PullSecretId) -> Self {
        Self {
            client: CredentialClient::new(client, CredentialProvider::Ssh, tenant),
            pull_secret_id,
        }
    }

    /// Asks the broker for a token scoped to the session's lease. The ephemeral SSH private key never leaves the
    /// gateway.
    pub async fn register_session(
        &self,
        session_id: &SshSessionId,
        lease_id: &LeaseId,
        ttl_seconds: NonZeroU32,
        requester: ServiceAccountId,
    ) -> Result<SessionCredential, ClientError> {
        let issued = self
            .client
            .issue(IssueParams {
                pull_secret_id: self.pull_secret_id.clone(),
                scope: CredentialScope::SshSession {
                    lease_id: lease_id.clone(),
                },
                ttl_seconds,
                requester: Some(requester),
            })
            .await?;
        Ok(SessionCredential {
            token_id: issued.token_id,
            lease_id: lease_id.clone(),
            session_id: session_id.clone(),
        })
    }

    /// Revokes the session's token. A broker that refuses, answers late, or answers garbage is logged and
    /// tolerated, because the token expires on its own; only a request that was never delivered is an error.
    pub async fn revoke_session(&self, credential: &SessionCredential, reason: &str) -> Result<(), ClientError> {
        match self.client.revoke(credential.token_id.clone(), reason).await {
            Ok(_) => Ok(()),
            Err(error @ (ClientError::Transport(_) | ClientError::Encode(_))) => Err(error),
            Err(error) => {
                warn!(%error, token_id = %credential.token_id, "credential revoke failed (best effort)");
                Ok(())
            }
        }
    }
}
