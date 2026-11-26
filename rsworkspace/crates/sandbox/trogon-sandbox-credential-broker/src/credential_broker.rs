use std::sync::Arc;
use std::time::SystemTime;

use async_nats::jetstream;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    CredentialAck, CredentialAuditEvent, CredentialError, CredentialId, CredentialIssueRequest,
    CredentialIssueResponse, CredentialProvider, CredentialRevokeRequest, CredentialRotateRequest, ServiceAccountId,
    TenantId, TokenId,
};

use crate::{
    AuditLogger, BrokerSetupError, CommittedPolicyStore, CredentialMaterial, CredentialStore, CredentialStoreError,
    CredentialSummary, FenceMode, FenceRefusal, JetStreamAuditLogger, KmsProvider, MigrationReport,
    NatsKvCommittedPolicies, NatsKvCredentialStore, NatsKvTokenStore, TokenStore, check_fence,
};

#[cfg(test)]
mod tests;

fn backend(error: impl std::fmt::Display) -> CredentialError {
    CredentialError::Backend {
        message: error.to_string(),
    }
}

/// Stores provider credentials and issues short-lived scoped tokens from them.
pub struct CredentialBroker {
    pub credentials: Arc<dyn CredentialStore>,
    pub tokens: Arc<dyn TokenStore>,
    pub audit: Arc<dyn AuditLogger>,
    policies: Option<Arc<dyn CommittedPolicyStore>>,
    fence_mode: FenceMode,
}

impl CredentialBroker {
    pub fn new(
        credentials: Arc<dyn CredentialStore>,
        tokens: Arc<dyn TokenStore>,
        audit: Arc<dyn AuditLogger>,
    ) -> Self {
        Self {
            credentials,
            tokens,
            audit,
            policies: None,
            fence_mode: FenceMode::default(),
        }
    }

    /// A broker over NATS: credential and token buckets, the audit stream and the committed-policy bucket.
    pub async fn jetstream(
        jetstream: jetstream::Context,
        kms: Arc<dyn KmsProvider>,
        allow_legacy_credentials: bool,
        fence_mode: FenceMode,
    ) -> Result<Self, BrokerSetupError> {
        let credentials =
            NatsKvCredentialStore::new(jetstream.clone(), kms).with_legacy_reads(allow_legacy_credentials);
        let tokens = NatsKvTokenStore::new(NatsKvTokenStore::open_bucket(&jetstream).await?);
        let audit = JetStreamAuditLogger::new(jetstream.clone()).await?;
        let policies = NatsKvCommittedPolicies::new(NatsKvCommittedPolicies::open_bucket(&jetstream).await?);
        Ok(Self::new(Arc::new(credentials), Arc::new(tokens), Arc::new(audit))
            .with_policy_fence(Arc::new(policies), fence_mode))
    }

    /// Resolves fenced requests against `policies` and applies `mode` to requests that carry no fence. Without a
    /// store, any request that does carry a fence is refused.
    pub fn with_policy_fence(mut self, policies: Arc<dyn CommittedPolicyStore>, mode: FenceMode) -> Self {
        self.policies = Some(policies);
        self.fence_mode = mode;
        self
    }

    /// Sets how an unfenced request is treated without changing the store.
    pub fn with_fence_mode(mut self, mode: FenceMode) -> Self {
        self.fence_mode = mode;
        self
    }

    pub async fn issue(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        request: CredentialIssueRequest,
    ) -> Result<CredentialIssueResponse, CredentialError> {
        check_fence(
            self.policies.as_deref(),
            self.fence_mode,
            tenant,
            request.fence.as_ref(),
            &request.pull_secret_id,
        )
        .await
        .map_err(|refusal| {
            if refusal == FenceRefusal::Missing {
                warn!(%provider, %tenant, "refused an unfenced credential request");
            }
            CredentialError::PolicyFenceRefused {
                message: refusal.to_string(),
            }
        })?;
        let credential_id = self
            .credentials
            .resolve_pull_secret(provider, &request.pull_secret_id)
            .await
            .map_err(backend)?;
        let material = self
            .credentials
            .get_material(provider, tenant, &credential_id)
            .await
            .map_err(|_| CredentialError::NotFound {
                message: "credential not found".to_string(),
            })?;
        let token_value = material
            .issue_for_scope(&request.scope)
            .map_err(|error| CredentialError::ScopeDenied {
                message: error.to_string(),
            })?;
        let response = self
            .tokens
            .issue(&credential_id, &request.scope, token_value, request.ttl_seconds)
            .await
            .map_err(backend)?;
        let requester = request.requester.unwrap_or_else(|| ServiceAccountId::new("unknown"));
        self.audit(CredentialAuditEvent::Issued {
            tenant: tenant.clone(),
            credential_id: credential_id.clone(),
            token_id: response.token_id.clone(),
            scope: request.scope,
            ttl_seconds: request.ttl_seconds,
            requester,
            at: SystemTime::now(),
        })
        .await;
        info!(%provider, %tenant, token_id = %response.token_id, %credential_id, "issued credential token");
        Ok(response)
    }

    pub async fn revoke(&self, request: CredentialRevokeRequest) -> Result<CredentialAck, CredentialError> {
        self.tokens.revoke(&request.token_id).await.map_err(backend)?;
        self.audit(CredentialAuditEvent::Revoked {
            token_id: request.token_id.clone(),
            reason: request.reason,
            at: SystemTime::now(),
        })
        .await;
        info!(token_id = %request.token_id, "revoked credential token");
        Ok(CredentialAck { message: None })
    }

    pub async fn rotate(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        request: CredentialRotateRequest,
        material: CredentialMaterial,
    ) -> Result<CredentialAck, CredentialError> {
        let version = self
            .credentials
            .rotate_credential(provider, tenant, &request.credential_id, &material)
            .await
            .map_err(backend)?;
        Ok(self.audit_rotated(provider, &request.credential_id, version).await)
    }

    /// Rewrites a stored credential in the current envelope format. Envelope records carry their own tenant;
    /// legacy ones need [`Self::reencrypt_for`].
    pub async fn reencrypt(
        &self,
        provider: CredentialProvider,
        request: CredentialRotateRequest,
    ) -> Result<CredentialAck, CredentialError> {
        self.reencrypt_for(provider, None, request).await
    }

    pub async fn reencrypt_for(
        &self,
        provider: CredentialProvider,
        tenant: Option<&TenantId>,
        request: CredentialRotateRequest,
    ) -> Result<CredentialAck, CredentialError> {
        let version = self
            .credentials
            .reencrypt_credential(provider, tenant, &request.credential_id)
            .await
            .map_err(backend)?;
        Ok(self.audit_rotated(provider, &request.credential_id, version).await)
    }

    /// Rewrites every legacy (no-AAD) record of `provider` as an envelope record owned by `tenant`.
    ///
    /// Safe to re-run and to resume after a crash: the set of legacy records is re-read on every call, so records
    /// already migrated are not listed, and a failure on one record does not stop the rest. With `dry_run`
    /// nothing is written. A legacy record names no tenant, so when several tenants own legacy records, pass
    /// `only` to migrate each tenant's ids in its own run; an empty `only` takes them all.
    pub async fn migrate_legacy(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        only: &[CredentialId],
        dry_run: bool,
    ) -> Result<MigrationReport, CredentialStoreError> {
        let mut report = MigrationReport::default();
        for credential_id in self.credentials.list_legacy(provider).await? {
            if !only.is_empty() && !only.contains(&credential_id) {
                continue;
            }
            if dry_run {
                report.would_migrate.push(credential_id);
                continue;
            }
            let request = CredentialRotateRequest {
                credential_id: credential_id.clone(),
            };
            match self.reencrypt_for(provider, Some(tenant), request).await {
                Ok(_) => report.migrated.push(credential_id),
                Err(error) => report.failed.push((credential_id, error)),
            }
        }
        report.remaining = self.credentials.list_legacy(provider).await?.len();
        Ok(report)
    }

    pub async fn validate_token(&self, token_id: &TokenId) -> Result<(), CredentialError> {
        self.tokens.validate(token_id).await.map(|_| ())
    }

    pub async fn store_credential(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        self.credentials
            .put_credential(provider, tenant, credential_id, &material)
            .await
    }

    pub async fn list_credentials(
        &self,
        provider: CredentialProvider,
    ) -> Result<Vec<CredentialSummary>, CredentialStoreError> {
        self.credentials.list_credentials(provider).await
    }

    async fn audit_rotated(
        &self,
        provider: CredentialProvider,
        credential_id: &CredentialId,
        version: u32,
    ) -> CredentialAck {
        self.audit(CredentialAuditEvent::Rotated {
            credential_id: credential_id.clone(),
            new_version: version,
            at: SystemTime::now(),
        })
        .await;
        info!(%provider, %credential_id, version, "rotated credential");
        CredentialAck {
            message: Some(format!("version {version}")),
        }
    }

    /// Records an event. A failure is logged and does not fail the request the event describes.
    async fn audit(&self, event: CredentialAuditEvent) {
        if let Err(error) = self.audit.emit(event).await {
            warn!(%error, "failed to emit credential audit event");
        }
    }
}
