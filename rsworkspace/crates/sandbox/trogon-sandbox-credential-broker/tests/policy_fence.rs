use std::collections::HashSet;
use std::error::Error;
use std::io;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, PolicyFence, PullSecretId, SandboxSpec, TenantId};
use trogon_sandbox_credential_broker::{
    BackendError, CommittedFence, CommittedFenceError, CommittedPolicy, CommittedPolicyStore, CommittedPolicyWriter,
    FenceMode, FenceModeError, FenceRefusal, InMemoryCommittedPolicies, PolicyStoreError, StaleCommit, check_fence,
};

struct DownPolicies;

#[async_trait]
impl CommittedPolicyStore for DownPolicies {
    async fn committed(
        &self,
        _tenant: &TenantId,
        _lease: &LeaseId,
    ) -> Result<Option<CommittedPolicy>, PolicyStoreError> {
        Err(BackendError::new(io::Error::other("down")).into())
    }
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("t")?)
}

fn lease() -> LeaseId {
    LeaseId::new("l1")
}

fn reference(name: &str) -> PullSecretId {
    PullSecretId::new(name)
}

fn fence(revision: u64) -> PolicyFence {
    PolicyFence {
        lease_id: lease(),
        policy_revision: revision,
    }
}

fn spec(with_credentials: bool) -> Result<SandboxSpec, Box<dyn Error>> {
    let mut wire: Value = json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": false,
        "boot_mode": "entrypoint_default",
    });
    if with_credentials {
        let extra = json!({
            "secret_bindings": [{
                "name": "A",
                "target": { "env": "A" },
                "mode": "mask",
                "inject_hosts": ["a.example.com"],
                "source": {
                    "provider": "registry",
                    "pull_secret_id": "pull-a",
                    "scope": { "kind": "registry_pull", "repository_glob": "*" },
                    "ttl_seconds": 60,
                },
            }],
            "network_policy": { "user_defined": {
                "rules": [{ "domain": "a.example.com", "action": "pass_through" }],
                "allow_ranges": [],
                "deny_ranges": [],
            } },
        });
        if let (Some(base), Some(extra)) = (wire.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
    }
    Ok(serde_json::from_value(wire)?)
}

#[test]
fn fence_modes_parse_and_default_to_optional() {
    assert_eq!("optional".parse(), Ok(FenceMode::Optional));
    assert_eq!("required".parse(), Ok(FenceMode::Required));
    assert_eq!("strict".parse::<FenceMode>(), Err(FenceModeError));
    assert_eq!(FenceMode::default(), FenceMode::Optional);
    assert_eq!(
        FenceModeError.to_string(),
        "unknown fence mode, expected 'optional' or 'required'"
    );
}

#[test]
fn refusals_and_errors_explain_themselves() -> Result<(), Box<dyn Error>> {
    assert_eq!(FenceRefusal::Missing.to_string(), "a policy fence is required");
    assert_eq!(
        FenceRefusal::NotCommitted.to_string(),
        "credential reference is not named by the committed policy"
    );
    assert_eq!(FenceRefusal::Unavailable.to_string(), "the policy store is unavailable");
    let stale = StaleCommit { current: 3, offered: 2 };
    assert_eq!(
        stale.to_string(),
        "revision 2 is not newer than the committed revision 3"
    );
    assert_eq!(
        PolicyStoreError::Contended.to_string(),
        "the committed policy for the lease kept changing; giving up"
    );
    assert_eq!(
        PolicyStoreError::from(stale).to_string(),
        "revision 2 is not newer than the committed revision 3"
    );
    let codec = PolicyStoreError::from(serde_json::from_str::<u8>("x").err().ok_or("parsed")?);
    assert_eq!(codec.to_string(), "committed policy could not be encoded or decoded");
    assert!(!CommittedFenceError::NotCommitted.to_string().is_empty());
    assert_eq!(
        CommittedFenceError::Unavailable(PolicyStoreError::Contended).to_string(),
        "reading the committed credential policy failed"
    );
    Ok(())
}

#[tokio::test]
async fn check_fence_decides_by_mode_store_lease_revision_and_reference() -> Result<(), Box<dyn Error>> {
    let policies = InMemoryCommittedPolicies::new();
    policies.commit_revision(tenant()?, lease(), 2, [reference("a")])?;
    let store: Option<&dyn CommittedPolicyStore> = Some(&policies);
    let t = tenant()?;
    let go = |store, mode, fence: Option<&PolicyFence>, name: &str| {
        let reference = reference(name);
        let t = t.clone();
        let fence = fence.cloned();
        async move { check_fence(store, mode, &t, fence.as_ref(), &reference).await }
    };
    assert_eq!(go(store, FenceMode::Optional, None, "a").await, Ok(()));
    assert_eq!(
        go(store, FenceMode::Required, None, "a").await,
        Err(FenceRefusal::Missing)
    );
    assert_eq!(go(store, FenceMode::Optional, Some(&fence(2)), "a").await, Ok(()));
    assert_eq!(
        go(store, FenceMode::Optional, Some(&fence(1)), "a").await,
        Err(FenceRefusal::NotCommitted)
    );
    assert_eq!(
        go(store, FenceMode::Optional, Some(&fence(3)), "a").await,
        Err(FenceRefusal::NotCommitted)
    );
    assert_eq!(
        go(store, FenceMode::Optional, Some(&fence(2)), "b").await,
        Err(FenceRefusal::NotCommitted)
    );
    assert_eq!(
        go(None, FenceMode::Optional, Some(&fence(2)), "a").await,
        Err(FenceRefusal::NotCommitted)
    );
    let down: Option<&dyn CommittedPolicyStore> = Some(&DownPolicies);
    assert_eq!(
        go(down, FenceMode::Optional, Some(&fence(2)), "a").await,
        Err(FenceRefusal::Unavailable)
    );
    Ok(())
}

#[tokio::test]
async fn the_in_memory_writer_bumps_the_revision_only_when_the_set_changes() -> Result<(), Box<dyn Error>> {
    let policies = InMemoryCommittedPolicies::new();
    let set = |names: &[&str]| -> HashSet<PullSecretId> { names.iter().map(|name| reference(name)).collect() };
    let t = tenant()?;
    assert_eq!(policies.commit(&t, &lease(), set(&["a"])).await?, 1);
    assert_eq!(policies.commit(&t, &lease(), set(&["a"])).await?, 1);
    assert_eq!(policies.commit(&t, &lease(), set(&["a", "b"])).await?, 2);
    let committed = policies.committed(&t, &lease()).await?.ok_or("committed")?;
    assert_eq!(committed.revision, 2);
    policies.revoke(&t, &lease()).await?;
    assert_eq!(policies.committed(&t, &lease()).await?, None);
    assert_eq!(policies.commit(&t, &lease(), set(&["a"])).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_replayed_revision_is_stale() -> Result<(), Box<dyn Error>> {
    let policies = InMemoryCommittedPolicies::new();
    policies.commit_revision(tenant()?, lease(), 2, [reference("a")])?;
    let stale = policies.commit_revision(tenant()?, lease(), 2, [reference("b")]);
    assert_eq!(stale, Err(StaleCommit { current: 2, offered: 2 }));
    Ok(())
}

#[tokio::test]
async fn a_committed_fence_stamps_the_committed_revision() -> Result<(), Box<dyn Error>> {
    let policies = Arc::new(InMemoryCommittedPolicies::new());
    policies.commit_revision(tenant()?, lease(), 4, [reference("pull-a")])?;
    let relaxed = CommittedFence::new(policies.clone(), false);
    assert_eq!(relaxed.resolve(&tenant()?, &lease(), &spec(false)?).await?, None);
    assert_eq!(
        relaxed.resolve(&tenant()?, &lease(), &spec(true)?).await?,
        Some(fence(4))
    );
    let missing = relaxed
        .resolve(&tenant()?, &LeaseId::new("other"), &spec(true)?)
        .await?;
    assert_eq!(missing, None);
    Ok(())
}

#[tokio::test]
async fn a_required_fence_refuses_a_lease_without_a_committed_policy() -> Result<(), Box<dyn Error>> {
    let policies = Arc::new(InMemoryCommittedPolicies::new());
    let required = CommittedFence::new(policies, true);
    let result = required.resolve_lease(&tenant()?, &lease()).await;
    assert!(matches!(result, Err(CommittedFenceError::NotCommitted)));
    Ok(())
}

#[tokio::test]
async fn an_unreadable_policy_store_is_fatal_only_when_required() -> Result<(), Box<dyn Error>> {
    let down = Arc::new(DownPolicies);
    let relaxed = CommittedFence::new(down.clone(), false);
    assert_eq!(relaxed.resolve_lease(&tenant()?, &lease()).await?, None);
    let required = CommittedFence::new(down, true);
    let result = required.resolve_lease(&tenant()?, &lease()).await;
    assert!(matches!(result, Err(CommittedFenceError::Unavailable(_))));
    Ok(())
}
