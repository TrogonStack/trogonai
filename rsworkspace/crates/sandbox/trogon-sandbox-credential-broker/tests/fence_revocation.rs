use std::error::Error;
use std::sync::Arc;

use serde_json::json;
use trogon_sandbox_contract::{LeaseId, PolicyFence, PullSecretId, SandboxSpec, TenantId};
use trogon_sandbox_credential_broker::{
    CommittedFence, CommittedPolicyStore, CommittedPolicyWriter, FenceMode, FenceRefusalError,
    InMemoryCommittedPolicies, check_fence,
};

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("t")?)
}

fn lease() -> LeaseId {
    LeaseId::new("l1")
}

fn credential_free_spec() -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": false,
        "boot_mode": "entrypoint_default",
    }))?)
}

#[tokio::test]
async fn a_required_fence_lets_a_lease_without_credentials_through() -> Result<(), Box<dyn Error>> {
    let policies = Arc::new(InMemoryCommittedPolicies::new());
    let required = CommittedFence::new(policies, true);
    let fence = required.resolve(&tenant()?, &lease(), &credential_free_spec()?).await?;
    assert_eq!(fence, None);
    Ok(())
}

#[tokio::test]
async fn revoking_an_absent_record_is_repeatable() -> Result<(), Box<dyn Error>> {
    let policies = InMemoryCommittedPolicies::new();
    let t = tenant()?;
    policies.commit(&t, &lease(), [PullSecretId::new("a")].into()).await?;
    policies.revoke(&t, &lease()).await?;
    assert_eq!(policies.committed(&t, &lease()).await?, None);
    policies.revoke(&t, &lease()).await?;
    assert_eq!(policies.committed(&t, &lease()).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_revoked_lease_resolves_nothing() -> Result<(), Box<dyn Error>> {
    let policies = InMemoryCommittedPolicies::new();
    let t = tenant()?;
    policies.commit_revision(t.clone(), lease(), 3, [PullSecretId::new("a")])?;
    policies.revoke(&t, &lease()).await?;
    let fence = PolicyFence {
        lease_id: lease(),
        policy_revision: 3,
    };
    let store: Option<&dyn CommittedPolicyStore> = Some(&policies);
    let refused = check_fence(store, FenceMode::Optional, &t, Some(&fence), &PullSecretId::new("a")).await;
    assert_eq!(refused, Err(FenceRefusalError::NotCommitted));
    Ok(())
}
