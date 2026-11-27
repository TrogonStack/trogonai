use super::*;
use trogon_sandbox_contract::{
    CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform, DomainRule,
    DomainRuleAction, HeaderName, NetworkPolicy, PullSecretId,
};
use trogon_sandbox_credential_broker::{
    CommittedPolicyStore, CommittedPolicyWriter, InMemoryCommittedPolicies, PolicyStoreError,
};

fn transform_policy(references: &[&str]) -> anyhow::Result<NetworkPolicy> {
    let rules = references
        .iter()
        .enumerate()
        .map(|(index, reference)| {
            Ok(DomainRule {
                domain: format!("api{index}.example.com").parse()?,
                action: DomainRuleAction::Transform(CredentialTransform {
                    match_rules: vec![],
                    credential: CredentialRequest {
                        provider: CredentialProvider::Registry,
                        pull_secret_id: PullSecretId::new(*reference),
                        scope: CredentialScope::RegistryPull {
                            repository_glob: "*".to_string(),
                        },
                        ttl_seconds: 60,
                        requester: None,
                    },
                    injection: CredentialInjection::Header(HeaderName::new("authorization")?),
                }),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(NetworkPolicy::UserDefined {
        rules,
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn references(names: &[&str]) -> std::collections::HashSet<PullSecretId> {
    names.iter().map(|n| PullSecretId::new(*n)).collect()
}

fn core_committing(response: Bytes) -> anyhow::Result<(BrokerCore, Arc<InMemoryCommittedPolicies>)> {
    let policies = Arc::new(InMemoryCommittedPolicies::new());
    let core = make_core(response)?.with_committed_policy_writer(policies.clone());
    Ok((core, policies))
}

async fn grant(core: &BrokerCore, key: &str, policy: NetworkPolicy) -> Result<LeaseId, Box<dyn Error>> {
    let mut req = edit_request_spec(make_request(vec![ProviderKind::Mock])?, |parts| {
        parts.network_policy = Some(policy);
    })?;
    req.idempotency_key = LeaseRequestKey::new(key)?;
    match core.handle_lease_request(req).await {
        LeaseResponse::Granted(granted) => Ok(granted.lease_id),
        other => Err(format!("expected a grant, got {other:?}").into()),
    }
}

#[tokio::test]
async fn a_new_lease_commits_revision_one_with_the_spec_references() -> TestResult {
    let (core, policies) = core_committing(mock_provider_response()?)?;
    let lease = grant(&core, "commit-create", transform_policy(&["a", "b"])?).await?;

    let committed = policies
        .committed(&TenantId::new("test-tenant")?, &lease)
        .await?
        .ok_or("the policy is committed before the lease is granted")?;
    assert_eq!(committed.revision, 1);
    assert_eq!(committed.references, references(&["a", "b"]));
    Ok(())
}

#[tokio::test]
async fn a_lease_without_credential_rules_still_commits_an_empty_policy() -> TestResult {
    let (core, policies) = core_committing(mock_provider_response()?)?;
    let lease = grant(&core, "commit-empty", NetworkPolicy::AllowAll).await?;

    let committed = policies
        .committed(&TenantId::new("test-tenant")?, &lease)
        .await?
        .ok_or("no committed policy")?;
    assert_eq!(committed.revision, 1);
    assert!(committed.references.is_empty());
    Ok(())
}

#[tokio::test]
async fn every_policy_update_commits_a_higher_revision() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease = LeaseId::new("lease-under-test");
    let (core, policies) = core_committing(network_policy_applied_response()?)?;
    core.active_leases.put(&running_source_record(&tenant, &lease)?).await?;

    for (expected, names) in [(1, vec!["a"]), (2, vec!["a", "b"]), (3, vec!["c"])] {
        let response = core
            .handle_network_policy_update(make_network_policy_update_request(&lease, transform_policy(&names)?))
            .await;
        assert!(
            matches!(response, NetworkPolicyUpdateResponse::Applied),
            "expected Applied, got {response:?}"
        );
        let committed = policies
            .committed(&tenant, &lease)
            .await?
            .ok_or("no committed policy")?;
        assert_eq!(committed.revision, expected);
        assert_eq!(committed.references, references(&names));
    }
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_deletes_its_committed_policy() -> TestResult {
    let (core, policies) = core_committing(mock_provider_response()?)?;
    let lease = grant(&core, "commit-release", transform_policy(&["a"])?).await?;
    let tenant = TenantId::new("test-tenant")?;
    assert!(policies.committed(&tenant, &lease).await?.is_some());

    core.handle_cancel(lease.clone(), LeaseReleasedReason::Cancelled)
        .await?;

    assert!(policies.committed(&tenant, &lease).await?.is_none());
    Ok(())
}

struct FailingWriter;

#[async_trait::async_trait]
impl CommittedPolicyWriter for FailingWriter {
    async fn commit(
        &self,
        _tenant: &TenantId,
        _lease: &LeaseId,
        _references: std::collections::HashSet<PullSecretId>,
    ) -> Result<u64, PolicyStoreError> {
        Err(PolicyStoreError::Contended)
    }

    async fn revoke(&self, _tenant: &TenantId, _lease: &LeaseId) -> Result<(), PolicyStoreError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_failing_policy_store_denies_the_lease_instead_of_dispatching_it() -> TestResult {
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let core = make_core_with_bus(bus.clone())?.with_committed_policy_writer(Arc::new(FailingWriter));
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.idempotency_key = LeaseRequestKey::new("commit-fails")?;

    let response = core.handle_lease_request(req).await;

    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::Backend { .. })),
        "expected a backend denial, got {response:?}"
    );
    assert!(
        locked(&bus.last_payload)?.is_none(),
        "nothing is dispatched when the policy cannot be committed"
    );
    Ok(())
}

#[tokio::test]
async fn a_failing_policy_store_denies_a_policy_update() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease = LeaseId::new("lease-under-test");
    let core = make_core(network_policy_applied_response()?)?.with_committed_policy_writer(Arc::new(FailingWriter));
    core.active_leases.put(&running_source_record(&tenant, &lease)?).await?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease, transform_policy(&["a"])?))
        .await;

    assert!(matches!(
        response,
        NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn without_a_writer_nothing_is_committed_and_leases_still_grant() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    grant(&core, "no-writer", transform_policy(&["a"])?).await?;
    Ok(())
}
