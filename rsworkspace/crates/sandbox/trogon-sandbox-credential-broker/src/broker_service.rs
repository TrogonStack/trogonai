use std::sync::Arc;

use serde::Serialize;
use trogon_sandbox_contract::{
    CredentialAckOutcome, CredentialIssueOutcome, CredentialIssueRequest, CredentialProvider, CredentialRevokeRequest,
    CredentialRotateRequest, TenantId,
};

use crate::{CredentialBroker, ServiceError, ServiceSubscriptions};

pub(crate) const ISSUE_PREFIX: &str = "sandbox.credentials.issue.";
pub(crate) const REVOKE_SUBJECT: &str = "sandbox.credentials.revoke.>";
pub(crate) const ROTATE_PREFIX: &str = "sandbox.credentials.rotate.";
pub(crate) const ISSUE_SUBJECT: &str = "sandbox.credentials.issue.>";
pub(crate) const ROTATE_SUBJECT: &str = "sandbox.credentials.rotate.>";

fn encode(value: &impl Serialize) -> Result<Vec<u8>, ServiceError> {
    serde_json::to_vec(value).map_err(ServiceError::Encode)
}

/// Serves the credential broker over NATS request and reply.
///
/// Each handler takes the request subject and payload and returns the reply payload. A refusal by the broker is
/// part of the reply; an error here means the request itself was malformed.
pub struct BrokerService {
    broker: Arc<CredentialBroker>,
}

impl BrokerService {
    pub fn new(broker: Arc<CredentialBroker>) -> Self {
        Self { broker }
    }

    /// Subscribes to the request subjects and waits until the server has them, so a request sent afterwards is
    /// never answered with no responders.
    pub async fn subscribe(self: Arc<Self>, client: async_nats::Client) -> Result<ServiceSubscriptions, ServiceError> {
        let issue = client.subscribe(ISSUE_SUBJECT).await?;
        let revoke = client.subscribe(REVOKE_SUBJECT).await?;
        let rotate = client.subscribe(ROTATE_SUBJECT).await?;
        client.flush().await?;
        Ok(ServiceSubscriptions::new(self, client, issue, revoke, rotate))
    }

    /// Issues a token for the provider and tenant the subject names.
    pub async fn issue(&self, subject: &str, payload: &[u8]) -> Result<Vec<u8>, ServiceError> {
        let rest = subject
            .strip_prefix(ISSUE_PREFIX)
            .ok_or(ServiceError::UnexpectedSubject)?;
        let (provider, tenant) = rest.split_once('.').ok_or(ServiceError::MissingTenant)?;
        let provider: CredentialProvider = provider.parse()?;
        let tenant = TenantId::new(tenant)?;
        let request: CredentialIssueRequest = serde_json::from_slice(payload)?;
        let outcome = match self.broker.issue(provider, &tenant, request).await {
            Ok(response) => CredentialIssueOutcome::Ok(response),
            Err(error) => CredentialIssueOutcome::Err(error),
        };
        encode(&outcome)
    }

    pub async fn revoke(&self, payload: &[u8]) -> Result<Vec<u8>, ServiceError> {
        let request: CredentialRevokeRequest = serde_json::from_slice(payload)?;
        let outcome = match self.broker.revoke(request).await {
            Ok(ack) => CredentialAckOutcome::Ok(ack),
            Err(error) => CredentialAckOutcome::Err(error),
        };
        encode(&outcome)
    }

    /// Re-encrypts a stored credential of the provider the subject names.
    pub async fn rotate(&self, subject: &str, payload: &[u8]) -> Result<Vec<u8>, ServiceError> {
        let provider = subject
            .strip_prefix(ROTATE_PREFIX)
            .ok_or(ServiceError::UnexpectedSubject)?
            .trim_end_matches('.');
        let provider: CredentialProvider = provider.parse()?;
        let request: CredentialRotateRequest = serde_json::from_slice(payload)?;
        let outcome = match self.broker.reencrypt(provider, request).await {
            Ok(ack) => CredentialAckOutcome::Ok(ack),
            Err(error) => CredentialAckOutcome::Err(error),
        };
        encode(&outcome)
    }
}
