use std::error::Error;
use std::io;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    CredentialAuditEvent, CredentialError, CredentialId, CredentialIssueRequest, CredentialProvider,
    CredentialRevokeRequest, CredentialRotateRequest, CredentialScope, LeaseId, PolicyFence, PullSecretId,
    ServiceAccountId, ShortLivedToken, TenantId, TokenId,
};
use trogon_sandbox_credential_broker::{
    AuditError, AuditLogger, BackendError, CommittedPolicy, CommittedPolicyStore, CredentialBroker, CredentialMaterial,
    CredentialStore, CredentialStoreError, CredentialSummary, FenceMode, InMemoryAuditLogger,
    InMemoryCommittedPolicies, InMemoryCredentialStore, InMemoryTokenStore, InProcessKey, PolicyStoreError,
    TokenRecord, TokenStore, TokenStoreError,
};

const REGISTRY: CredentialProvider = CredentialProvider::Registry;

fn down() -> BackendError {
    BackendError::new(io::Error::other("down"))
}

struct DownAudit;

#[async_trait]
impl AuditLogger for DownAudit {
    async fn emit(&self, _event: CredentialAuditEvent) -> Result<(), AuditError> {
        Err(down().into())
    }
}

struct DownTokens;

#[async_trait]
impl TokenStore for DownTokens {
    async fn issue(
        &self,
        _credential_id: &CredentialId,
        _scope: &CredentialScope,
        _token_value: ShortLivedToken,
        _ttl_seconds: NonZeroU32,
    ) -> Result<trogon_sandbox_contract::CredentialIssueResponse, TokenStoreError> {
        Err(down().into())
    }

    async fn validate(&self, _token_id: &TokenId) -> Result<TokenRecord, CredentialError> {
        Err(CredentialError::Backend {
            message: "down".to_string(),
        })
    }

    async fn revoke(&self, _token_id: &TokenId) -> Result<(), TokenStoreError> {
        Err(down().into())
    }
}

struct DownCredentials;

#[async_trait]
impl CredentialStore for DownCredentials {
    async fn put_credential(
        &self,
        _provider: CredentialProvider,
        _tenant: &TenantId,
        _credential_id: &CredentialId,
        _material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        Err(down().into())
    }

    async fn get_material(
        &self,
        _provider: CredentialProvider,
        _tenant: &TenantId,
        _credential_id: &CredentialId,
    ) -> Result<CredentialMaterial, CredentialStoreError> {
        Err(down().into())
    }

    async fn rotate_credential(
        &self,
        _provider: CredentialProvider,
        _tenant: &TenantId,
        _credential_id: &CredentialId,
        _material: &CredentialMaterial,
    ) -> Result<u32, CredentialStoreError> {
        Err(down().into())
    }

    async fn reencrypt_credential(
        &self,
        _provider: CredentialProvider,
        _tenant: Option<&TenantId>,
        _credential_id: &CredentialId,
    ) -> Result<u32, CredentialStoreError> {
        Err(down().into())
    }

    async fn list_legacy(&self, _provider: CredentialProvider) -> Result<Vec<CredentialId>, CredentialStoreError> {
        Err(down().into())
    }

    async fn map_pull_secret(
        &self,
        _provider: CredentialProvider,
        _pull_secret_id: &PullSecretId,
        _credential_id: &CredentialId,
    ) -> Result<(), CredentialStoreError> {
        Err(down().into())
    }

    async fn resolve_pull_secret(
        &self,
        _provider: CredentialProvider,
        _pull_secret_id: &PullSecretId,
    ) -> Result<CredentialId, CredentialStoreError> {
        Err(down().into())
    }

    async fn list_credentials(
        &self,
        _provider: CredentialProvider,
    ) -> Result<Vec<CredentialSummary>, CredentialStoreError> {
        Err(down().into())
    }
}

struct DownPolicies;

#[async_trait]
impl CommittedPolicyStore for DownPolicies {
    async fn committed(
        &self,
        _tenant: &TenantId,
        _lease: &LeaseId,
    ) -> Result<Option<CommittedPolicy>, PolicyStoreError> {
        Err(down().into())
    }
}

fn kms() -> Arc<InProcessKey> {
    Arc::new(InProcessKey::from_bytes([9u8; 32]))
}

fn broker() -> (CredentialBroker, Arc<InMemoryAuditLogger>) {
    let audit = Arc::new(InMemoryAuditLogger::new());
    let broker = CredentialBroker::new(
        Arc::new(InMemoryCredentialStore::new(kms())),
        Arc::new(InMemoryTokenStore::new()),
        audit.clone(),
    );
    (broker, audit)
}

fn tenant_a() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("tenant-a")?)
}

fn password(value: &str) -> CredentialMaterial {
    CredentialMaterial::RegistryBasic {
        username: "pull-user".to_string(),
        password: value.to_string(),
    }
}

async fn seeded(secret: &str) -> Result<(CredentialBroker, Arc<InMemoryAuditLogger>), Box<dyn Error>> {
    let (broker, audit) = broker();
    broker
        .store_credential(REGISTRY, &tenant_a()?, &CredentialId::new(secret), password("pass-v1"))
        .await?;
    Ok((broker, audit))
}

fn pull_scope() -> CredentialScope {
    CredentialScope::RegistryPull {
        repository_glob: "registry.example/*".to_string(),
    }
}

fn request(secret: &str, ttl: u32) -> Result<CredentialIssueRequest, Box<dyn Error>> {
    Ok(CredentialIssueRequest {
        pull_secret_id: PullSecretId::new(secret),
        scope: pull_scope(),
        ttl_seconds: NonZeroU32::new(ttl).ok_or("ttl")?,
        requester: None,
        fence: None,
    })
}

fn fenced(secret: &str, lease: &str, revision: u64) -> Result<CredentialIssueRequest, Box<dyn Error>> {
    Ok(CredentialIssueRequest {
        fence: Some(PolicyFence {
            lease_id: LeaseId::new(lease),
            policy_revision: revision,
        }),
        ..request(secret, 300)?
    })
}

fn refused(result: Result<impl std::fmt::Debug, CredentialError>) -> bool {
    matches!(result, Err(CredentialError::PolicyFenceRefused { .. }))
}

#[tokio::test]
async fn an_issued_token_validates_then_expires() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("secret-1").await?;
    let issued = broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 1)?).await?;
    assert_eq!(issued.token.expose_secret(), "pull-user:pass-v1");
    assert!(broker.validate_token(&issued.token_id).await.is_ok());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(matches!(
        broker.validate_token(&issued.token_id).await,
        Err(CredentialError::TokenExpired { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_revoked_token_no_longer_validates() -> Result<(), Box<dyn Error>> {
    let (broker, audit) = seeded("secret-1").await?;
    let issued = broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?).await?;
    let ack = broker
        .revoke(CredentialRevokeRequest {
            token_id: issued.token_id.clone(),
            reason: "test".to_string(),
        })
        .await?;
    assert_eq!(ack.message, None);
    assert!(matches!(
        broker.validate_token(&issued.token_id).await,
        Err(CredentialError::TokenRevoked { .. })
    ));
    let events = audit.events().await;
    assert!(matches!(events[0], CredentialAuditEvent::Issued { .. }));
    assert!(matches!(events[1], CredentialAuditEvent::Revoked { .. }));
    Ok(())
}

#[tokio::test]
async fn an_unknown_token_is_not_found() -> Result<(), Box<dyn Error>> {
    let (broker, _) = broker();
    assert!(matches!(
        broker.validate_token(&TokenId::new("nope")).await,
        Err(CredentialError::NotFound { .. })
    ));
    broker
        .revoke(CredentialRevokeRequest {
            token_id: TokenId::new("nope"),
            reason: String::new(),
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn rotation_keeps_old_tokens_valid_and_new_tokens_use_the_new_material() -> Result<(), Box<dyn Error>> {
    let (broker, audit) = seeded("secret-1").await?;
    let first = broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?).await?;
    let ack = broker
        .rotate(
            REGISTRY,
            &tenant_a()?,
            CredentialRotateRequest {
                credential_id: CredentialId::new("secret-1"),
            },
            password("pass-v2"),
        )
        .await?;
    assert_eq!(ack.message.as_deref(), Some("version 2"));
    assert!(broker.validate_token(&first.token_id).await.is_ok());
    let second = broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?).await?;
    assert_ne!(first.token, second.token);
    assert_eq!(second.token.expose_secret(), "pull-user:pass-v2");
    assert!(
        audit
            .events()
            .await
            .iter()
            .any(|event| matches!(event, CredentialAuditEvent::Rotated { new_version: 2, .. }))
    );
    Ok(())
}

#[tokio::test]
async fn reencrypting_an_envelope_record_bumps_its_version() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("secret-1").await?;
    let ack = broker
        .reencrypt(
            REGISTRY,
            CredentialRotateRequest {
                credential_id: CredentialId::new("secret-1"),
            },
        )
        .await?;
    assert_eq!(ack.message.as_deref(), Some("version 2"));
    let listed = broker.list_credentials(REGISTRY).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].version, 2);
    Ok(())
}

#[tokio::test]
async fn another_tenant_cannot_open_the_credential() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("secret-1").await?;
    let result = broker
        .issue(REGISTRY, &TenantId::new("tenant-b")?, request("secret-1", 300)?)
        .await;
    assert!(matches!(result, Err(CredentialError::NotFound { .. })));
    Ok(())
}

#[tokio::test]
async fn material_for_another_kind_of_scope_is_denied() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("secret-1").await?;
    let wrong_scope = CredentialIssueRequest {
        scope: CredentialScope::S3ReadOnly {
            bucket: "b".to_string(),
            prefix: String::new(),
        },
        ..request("secret-1", 300)?
    };
    let result = broker.issue(REGISTRY, &tenant_a()?, wrong_scope).await;
    assert!(matches!(result, Err(CredentialError::ScopeDenied { .. })));
    Ok(())
}

#[tokio::test]
async fn a_requester_is_recorded_in_the_audit_event() -> Result<(), Box<dyn Error>> {
    let (broker, audit) = seeded("secret-1").await?;
    let named = CredentialIssueRequest {
        requester: Some(ServiceAccountId::new("svc")),
        ..request("secret-1", 300)?
    };
    broker.issue(REGISTRY, &tenant_a()?, named).await?;
    broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?).await?;
    let events = audit.events().await;
    let requesters: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            CredentialAuditEvent::Issued { requester, .. } => Some(requester.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(requesters, vec!["svc".to_string(), "unknown".to_string()]);
    Ok(())
}

#[tokio::test]
async fn a_pull_secret_resolves_through_its_mapping() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("secret-1").await?;
    broker
        .credentials
        .map_pull_secret(REGISTRY, &PullSecretId::new("alias"), &CredentialId::new("secret-1"))
        .await?;
    let issued = broker.issue(REGISTRY, &tenant_a()?, request("alias", 300)?).await?;
    assert_eq!(issued.token.expose_secret(), "pull-user:pass-v1");
    Ok(())
}

#[tokio::test]
async fn a_failing_audit_log_does_not_fail_the_request() -> Result<(), Box<dyn Error>> {
    let broker = CredentialBroker::new(
        Arc::new(InMemoryCredentialStore::new(kms())),
        Arc::new(InMemoryTokenStore::new()),
        Arc::new(DownAudit),
    );
    broker
        .store_credential(REGISTRY, &tenant_a()?, &CredentialId::new("secret-1"), password("pw"))
        .await?;
    assert!(
        broker
            .issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?)
            .await
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn a_failing_token_store_is_a_backend_error() -> Result<(), Box<dyn Error>> {
    let credentials = Arc::new(InMemoryCredentialStore::new(kms()));
    let broker = CredentialBroker::new(credentials, Arc::new(DownTokens), Arc::new(InMemoryAuditLogger::new()));
    broker
        .store_credential(REGISTRY, &tenant_a()?, &CredentialId::new("secret-1"), password("pw"))
        .await?;
    let issued = broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?).await;
    assert!(matches!(issued, Err(CredentialError::Backend { .. })));
    let revoked = broker
        .revoke(CredentialRevokeRequest {
            token_id: TokenId::new("t"),
            reason: String::new(),
        })
        .await;
    assert!(matches!(revoked, Err(CredentialError::Backend { .. })));
    Ok(())
}

#[tokio::test]
async fn a_failing_credential_store_is_a_backend_error() -> Result<(), Box<dyn Error>> {
    let broker = CredentialBroker::new(
        Arc::new(DownCredentials),
        Arc::new(InMemoryTokenStore::new()),
        Arc::new(InMemoryAuditLogger::new()),
    );
    let issued = broker.issue(REGISTRY, &tenant_a()?, request("secret-1", 300)?).await;
    assert!(matches!(issued, Err(CredentialError::Backend { .. })));
    let rotate = CredentialRotateRequest {
        credential_id: CredentialId::new("secret-1"),
    };
    let rotated = broker
        .rotate(REGISTRY, &tenant_a()?, rotate.clone(), password("pw"))
        .await;
    assert!(matches!(rotated, Err(CredentialError::Backend { .. })));
    let reencrypted = broker.reencrypt(REGISTRY, rotate).await;
    assert!(matches!(reencrypted, Err(CredentialError::Backend { .. })));
    assert!(broker.migrate_legacy(REGISTRY, &tenant_a()?, &[], false).await.is_err());
    assert!(broker.list_credentials(REGISTRY).await.is_err());
    assert!(
        broker
            .store_credential(REGISTRY, &tenant_a()?, &CredentialId::new("c"), password("pw"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn migrating_with_nothing_legacy_is_a_no_op() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("secret-1").await?;
    let report = broker.migrate_legacy(REGISTRY, &tenant_a()?, &[], false).await?;
    assert!(report.migrated.is_empty() && report.failed.is_empty());
    assert_eq!(report.remaining, 0);
    Ok(())
}

async fn fenced_broker(mode: FenceMode) -> Result<(CredentialBroker, Arc<InMemoryCommittedPolicies>), Box<dyn Error>> {
    let policies = Arc::new(InMemoryCommittedPolicies::new());
    let (broker, _) = broker();
    let broker = broker.with_policy_fence(policies.clone(), mode);
    for id in ["cred-1", "cred-2"] {
        broker
            .store_credential(REGISTRY, &tenant_a()?, &CredentialId::new(id), password("pw"))
            .await?;
    }
    Ok((broker, policies))
}

#[tokio::test]
async fn a_request_at_the_committed_revision_resolves_only_a_named_reference() -> Result<(), Box<dyn Error>> {
    let (broker, policies) = fenced_broker(FenceMode::Required).await?;
    policies.commit_revision(tenant_a()?, LeaseId::new("l1"), 1, [PullSecretId::new("cred-1")])?;
    broker.issue(REGISTRY, &tenant_a()?, fenced("cred-1", "l1", 1)?).await?;
    assert!(refused(
        broker.issue(REGISTRY, &tenant_a()?, fenced("cred-2", "l1", 1)?).await
    ));
    Ok(())
}

#[tokio::test]
async fn a_stale_revision_is_refused_after_the_policy_is_edited() -> Result<(), Box<dyn Error>> {
    let (broker, policies) = fenced_broker(FenceMode::Optional).await?;
    let lease = LeaseId::new("l1");
    policies.commit_revision(tenant_a()?, lease.clone(), 1, [PullSecretId::new("cred-1")])?;
    policies.commit_revision(
        tenant_a()?,
        lease,
        2,
        [PullSecretId::new("cred-1"), PullSecretId::new("cred-2")],
    )?;
    assert!(refused(
        broker.issue(REGISTRY, &tenant_a()?, fenced("cred-1", "l1", 1)?).await
    ));
    broker.issue(REGISTRY, &tenant_a()?, fenced("cred-2", "l1", 2)?).await?;
    Ok(())
}

#[tokio::test]
async fn the_mode_decides_whether_an_unfenced_request_resolves() -> Result<(), Box<dyn Error>> {
    let (optional, _) = fenced_broker(FenceMode::Optional).await?;
    optional.issue(REGISTRY, &tenant_a()?, request("cred-1", 300)?).await?;
    let (required, _) = fenced_broker(FenceMode::Required).await?;
    assert!(refused(
        required.issue(REGISTRY, &tenant_a()?, request("cred-1", 300)?).await
    ));
    let relaxed = required.with_fence_mode(FenceMode::Optional);
    relaxed.issue(REGISTRY, &tenant_a()?, request("cred-1", 300)?).await?;
    Ok(())
}

#[tokio::test]
async fn another_tenant_cannot_use_a_lease_it_does_not_own() -> Result<(), Box<dyn Error>> {
    let (broker, policies) = fenced_broker(FenceMode::Optional).await?;
    policies.commit_revision(tenant_a()?, LeaseId::new("l1"), 1, [PullSecretId::new("cred-1")])?;
    let other = TenantId::new("tenant-b")?;
    assert!(refused(
        broker.issue(REGISTRY, &other, fenced("cred-1", "l1", 1)?).await
    ));
    Ok(())
}

#[tokio::test]
async fn a_fenced_request_without_a_policy_store_is_refused() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("cred-1").await?;
    assert!(refused(
        broker.issue(REGISTRY, &tenant_a()?, fenced("cred-1", "l1", 1)?).await
    ));
    Ok(())
}

#[tokio::test]
async fn an_unavailable_policy_store_refuses_a_fenced_request() -> Result<(), Box<dyn Error>> {
    let (broker, _) = seeded("cred-1").await?;
    let broker = broker.with_policy_fence(Arc::new(DownPolicies), FenceMode::Optional);
    assert!(refused(
        broker.issue(REGISTRY, &tenant_a()?, fenced("cred-1", "l1", 1)?).await
    ));
    Ok(())
}
