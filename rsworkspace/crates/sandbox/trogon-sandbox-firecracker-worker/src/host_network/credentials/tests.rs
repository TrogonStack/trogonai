use std::collections::HashSet;
use std::sync::Mutex;

use trogon_sandbox_contract::{
    CredentialInjection, CredentialScope, CredentialTransform, DomainPattern, DomainRule, ForwardUrl, HeaderName,
    PullSecretId, RequestForward,
};
use trogon_sandbox_credential_broker::InMemoryCommittedPolicies;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request_for(provider: CredentialProvider) -> CredentialRequest {
    CredentialRequest {
        provider,
        pull_secret_id: PullSecretId::new("ref"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".into(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

fn policy_for(providers: &[CredentialProvider]) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    let mut rules = Vec::new();
    for (index, provider) in providers.iter().enumerate() {
        rules.push(DomainRule {
            domain: DomainPattern::new(&format!("api{index}.example.com"))?,
            action: DomainRuleAction::Transform(CredentialTransform {
                match_rules: vec![],
                credential: request_for(*provider),
                injection: CredentialInjection::Header(HeaderName::new("X")?),
            }),
        });
    }
    Ok(NetworkPolicy::UserDefined {
        rules,
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

/// Records each connection and serves tokens that name the connection that made them.
#[derive(Default)]
struct RecordingConnector {
    calls: Mutex<Vec<(Vec<CredentialProvider>, Option<PolicyFence>)>>,
}

impl RecordingConnector {
    fn recorded(&self) -> Vec<(Vec<CredentialProvider>, Option<PolicyFence>)> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct Numbered(usize);

#[async_trait]
impl CredentialSource for Numbered {
    async fn fetch(&self, _request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        Ok(ShortLivedToken::new(format!("connection-{}", self.0)))
    }
}

#[async_trait]
impl CredentialConnector for RecordingConnector {
    async fn connect(
        &self,
        _tenant: &TenantId,
        providers: &[CredentialProvider],
        fence: Option<&PolicyFence>,
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        let mut calls = self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        calls.push((providers.to_vec(), fence.cloned()));
        Ok(Arc::new(Numbered(calls.len())))
    }
}

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("acme")?)
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

async fn fetched(credentials: &LeaseCredentials) -> Result<String, Box<dyn std::error::Error>> {
    let token = credentials
        .source
        .fetch(&request_for(CredentialProvider::Registry))
        .await?;
    Ok(token.expose_secret().to_string())
}

fn refreshable(
    connector: &Arc<RecordingConnector>,
    fence: Option<CommittedFence>,
) -> Result<LeaseCredentials, Box<dyn std::error::Error>> {
    Ok(LeaseCredentials::refreshable(CredentialRefresh {
        connector: connector.clone(),
        tenant: tenant()?,
        lease: lease(),
        fence,
    }))
}

#[tokio::test]
async fn a_policy_update_adds_a_provider_the_lease_started_without() -> TestResult {
    let connector = Arc::new(RecordingConnector::default());
    let mut credentials = refreshable(&connector, None)?;

    credentials.apply(&NetworkPolicy::AllowAll).await?;
    assert!(connector.recorded().is_empty());
    assert!(
        credentials
            .source
            .fetch(&request_for(CredentialProvider::Registry))
            .await
            .is_err()
    );

    credentials.apply(&policy_for(&[CredentialProvider::Registry])?).await?;
    assert_eq!(fetched(&credentials).await?, "connection-1");

    credentials.apply(&policy_for(&[CredentialProvider::S3])?).await?;
    let recorded = connector.recorded();
    assert_eq!(
        recorded.get(1).map(|call| call.0.clone()),
        Some(vec![CredentialProvider::Registry, CredentialProvider::S3]),
        "providers the lease already used stay connected"
    );
    assert_eq!(
        fetched(&credentials).await?,
        "connection-2",
        "the proxy's handle now reaches the new connection"
    );
    Ok(())
}

#[tokio::test]
async fn every_connection_carries_the_revision_committed_at_that_moment() -> TestResult {
    let store = Arc::new(InMemoryCommittedPolicies::new());
    let one = HashSet::from([PullSecretId::new("ref")]);
    let two = HashSet::from([PullSecretId::new("ref")]);
    store.commit_revision(tenant()?, lease(), 1, one)?;
    let connector = Arc::new(RecordingConnector::default());
    let mut credentials = refreshable(&connector, Some(CommittedFence::new(store.clone(), true)))?;
    let policy = policy_for(&[CredentialProvider::Registry])?;

    credentials.apply(&policy).await?;
    store.commit_revision(tenant()?, lease(), 2, two)?;
    credentials.apply(&policy).await?;

    let calls = connector.recorded();
    let revisions: Vec<_> = calls
        .iter()
        .map(|(_, fence)| fence.as_ref().map(|f| f.policy_revision))
        .collect();
    assert_eq!(revisions, vec![Some(1), Some(2)]);
    assert_eq!(
        calls
            .first()
            .and_then(|call| call.1.as_ref())
            .map(|fence| fence.lease_id.clone()),
        Some(lease())
    );
    Ok(())
}

#[tokio::test]
async fn a_required_fence_refuses_a_lease_with_no_committed_policy() -> TestResult {
    let store = Arc::new(InMemoryCommittedPolicies::new());
    let connector = Arc::new(RecordingConnector::default());
    let mut credentials = refreshable(&connector, Some(CommittedFence::new(store, true)))?;

    let result = credentials.apply(&policy_for(&[CredentialProvider::Registry])?).await;

    assert!(result.is_err());
    assert!(connector.recorded().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_fixed_source_cannot_gain_a_provider() -> TestResult {
    let mut credentials = LeaseCredentials::new(Arc::new(Numbered(0)), vec![]);
    credentials.apply(&NetworkPolicy::AllowAll).await?;
    let result = credentials.apply(&policy_for(&[CredentialProvider::Registry])?).await;
    assert!(result.is_err());
    Ok(())
}

#[test]
fn the_forward_identity_signs_with_the_configured_key_and_names_the_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("oidc.pem");
    std::fs::write(&path, rcgen::KeyPair::generate()?.serialize_pem())?;
    let identity = ForwardIdentity {
        signing_key_pem_file: path,
        tenant: tenant()?,
        lease: lease(),
    };

    let context = identity.context()?;

    assert_eq!(context.lease_id, lease());
    assert_eq!(context.tenant_id, tenant()?);
    assert_eq!(context.subject.as_str(), "lease-1");
    let missing = ForwardIdentity {
        signing_key_pem_file: dir.path().join("absent.pem"),
        ..identity
    };
    assert!(missing.context().is_err());
    Ok(())
}

#[test]
fn forwarding_is_detected_only_for_forward_rules() -> TestResult {
    assert!(!forwards_requests(&NetworkPolicy::AllowAll));
    assert!(!forwards_requests(&policy_for(&[CredentialProvider::S3])?));
    let forward = NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::Forward(RequestForward {
                forward_url: ForwardUrl::new("https://upstream.example")?,
                match_rules: vec![],
                ttl_seconds: 60,
            }),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    };
    assert!(forwards_requests(&forward));
    Ok(())
}
