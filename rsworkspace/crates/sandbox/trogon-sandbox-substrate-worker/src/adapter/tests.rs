//! Shared helpers for the provider tests, one module per behavior.

use std::error::Error;
use std::sync::Arc;

use trogon_sandbox_contract::{LeaseId, Port, TenantId};

use super::tests_support::{FakeControl, TestResult, provider_with};
use super::*;
use crate::actor_index::memory::MemoryActorIndex;

mod adoption_tests;
mod cancel_tests;
mod ensure_actor_tests;
mod fixture_tests;
mod lease_tests;
mod purge_named_tests;
mod resume_tests;
mod retention_tests;
mod validation_tests;

/// The UID the control plane reports for every actor a test creates.
const TEST_UID: &str = "00000000-0000-4000-8000-000000000001";

fn test_uid() -> Result<ActorUid, Box<dyn Error>> {
    Ok(ActorUid::try_from(TEST_UID.to_string())?)
}

fn tenant_a() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("tenant-a")?)
}

fn atespace() -> Result<Atespace, Box<dyn Error>> {
    Ok(Atespace::for_tenant(&tenant_a()?)?)
}

/// A provider over `control` whose actors carry [`TEST_UID`].
fn provider(control: Arc<FakeControl>) -> Result<SubstrateProvider, Box<dyn Error>> {
    control.set_uid(TEST_UID);
    provider_with(control, Arc::new(MemoryActorIndex::default()))
}

/// The lease an anonymous actor serves.
fn anonymous(lease_id: &LeaseId, persistent: bool, exposed_port: Option<Port>) -> Result<ActiveLease, Box<dyn Error>> {
    Ok(ActiveLease {
        atespace: atespace()?,
        actor: ActorName::for_lease(lease_id)?,
        named: false,
        persistent,
        exposed_port,
    })
}

/// The lease a named actor serves.
fn named(name: &str, persistent: bool) -> Result<ActiveLease, Box<dyn Error>> {
    let atespace = atespace()?;
    Ok(ActiveLease {
        actor: ActorName::for_named_lease(&atespace, name),
        atespace,
        named: true,
        persistent,
        exposed_port: None,
    })
}

fn object_ref(atespace: &str, name: &str) -> proto::ObjectRef {
    proto::ObjectRef {
        atespace: atespace.to_string(),
        name: name.to_string(),
    }
}

/// An index entry this provider owns, with a confirmed actor UID.
fn owned_entry(
    provider: &SubstrateProvider,
    lease_id: &LeaseId,
    lease: &ActiveLease,
) -> Result<ActorEntry, Box<dyn Error>> {
    let mut entry = claim(&lease.atespace, &lease.actor, lease_id);
    entry.owner = Some(ActorOwner {
        worker_id: provider.worker_id().clone(),
        claim_id: ActorClaimId::generate(),
        actor_uid: Some(test_uid()?),
    });
    Ok(entry)
}

/// Records `lease` in the index and in the provider as held and ready.
async fn adopt_test(
    provider: &SubstrateProvider,
    lease_id: &LeaseId,
    lease: ActiveLease,
) -> Result<ActorEntry, Box<dyn Error>> {
    let entry = owned_entry(provider, lease_id, &lease)?;
    provider.index.put(&entry).await?;
    provider.adopt(lease_id.clone(), lease, entry.clone(), true).await?;
    Ok(entry)
}

async fn indexed(provider: &SubstrateProvider, lease: &ActiveLease) -> Result<Option<ActorEntry>, Box<dyn Error>> {
    Ok(provider
        .index
        .get(lease.atespace.as_str(), lease.actor.as_str())
        .await?)
}

#[test]
fn the_fixture_uid_is_a_valid_actor_uid() -> TestResult {
    assert_eq!(test_uid()?.as_str(), TEST_UID);
    Ok(())
}
