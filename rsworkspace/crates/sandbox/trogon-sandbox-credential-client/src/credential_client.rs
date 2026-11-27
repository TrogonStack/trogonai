use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_nats::RequestClient;
use trogon_sandbox_contract::{
    CredentialAck, CredentialAckOutcome, CredentialId, CredentialIssueOutcome, CredentialIssueRequest,
    CredentialIssueResponse, CredentialProvider, CredentialRevokeRequest, CredentialRotateRequest, PolicyFence,
    TenantId, TokenId,
};
use trogon_sandbox_protocol::subjects::{credential_issue, credential_revoke, credential_rotate};

use crate::constants::DEFAULT_TIMEOUT;
use crate::{ClientError, IssueParams};

/// Requests tokens from the credential broker of one provider on behalf of one tenant.
#[derive(Debug, Clone)]
pub struct CredentialClient<C> {
    client: C,
    provider: CredentialProvider,
    tenant: TenantId,
    timeout: Duration,
    fence: Option<PolicyFence>,
}

impl<C: RequestClient> CredentialClient<C> {
    /// A client with a five second reply timeout and no policy fence.
    pub fn new(client: C, provider: CredentialProvider, tenant: TenantId) -> Self {
        Self {
            client,
            provider,
            tenant,
            timeout: DEFAULT_TIMEOUT,
            fence: None,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Stamps every issue request with the lease and committed policy revision this client acts for. The broker
    /// then resolves only references that revision names. Without it the client sends no fence, which a broker
    /// that requires the fence refuses.
    pub fn with_policy_fence(mut self, fence: PolicyFence) -> Self {
        self.fence = Some(fence);
        self
    }

    pub async fn issue(&self, params: IssueParams) -> Result<CredentialIssueResponse, ClientError> {
        let request = CredentialIssueRequest {
            pull_secret_id: params.pull_secret_id,
            scope: params.scope,
            ttl_seconds: params.ttl_seconds,
            requester: params.requester,
            fence: self.fence.clone(),
        };
        let subject = credential_issue(self.provider, &self.tenant);
        match self.call(subject, &request).await? {
            CredentialIssueOutcome::Ok(response) => Ok(response),
            CredentialIssueOutcome::Err(error) => Err(error.into()),
        }
    }

    pub async fn revoke(&self, token_id: TokenId, reason: impl Into<String>) -> Result<CredentialAck, ClientError> {
        let request = CredentialRevokeRequest {
            token_id,
            reason: reason.into(),
        };
        let subject = credential_revoke(self.provider, &self.tenant);
        self.ack(subject, &request).await
    }

    pub async fn rotate(&self, credential_id: CredentialId) -> Result<CredentialAck, ClientError> {
        let request = CredentialRotateRequest { credential_id };
        let subject = credential_rotate(self.provider);
        self.ack(subject, &request).await
    }

    async fn ack(&self, subject: String, request: &impl Serialize) -> Result<CredentialAck, ClientError> {
        match self.call(subject, request).await? {
            CredentialAckOutcome::Ok(ack) => Ok(ack),
            CredentialAckOutcome::Err(error) => Err(error.into()),
        }
    }

    async fn call<R: DeserializeOwned>(&self, subject: String, request: &impl Serialize) -> Result<R, ClientError> {
        let payload = serde_json::to_vec(request).map_err(ClientError::Encode)?;
        let reply = tokio::time::timeout(
            self.timeout,
            self.client
                .request_with_headers(subject, HeaderMap::new(), Bytes::from(payload)),
        )
        .await
        .map_err(|_| ClientError::Timeout(self.timeout))?
        .map_err(|error| ClientError::Transport(Box::new(error)))?;
        serde_json::from_slice(&reply.payload).map_err(ClientError::Decode)
    }
}
