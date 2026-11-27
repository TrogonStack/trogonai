use async_nats::jetstream::kv;
use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};

use super::entry_codec::{decode_entry, encode_entry};
use super::exchange_key::exchange_key;
use super::memory::MemoryActorIndex;
use super::nats_actor_index::{lost_delete, lost_update, settle};
use super::*;
use crate::naming::{ActorName, Atespace};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const UID: &str = "00000000-0000-4000-8000-000000000001";

fn atespace(tenant: &str) -> Result<Atespace, Box<dyn std::error::Error>> {
    Ok(Atespace::for_tenant(&TenantId::new(tenant)?)?)
}

fn owned(worker: &str) -> Result<ActorEntry, Box<dyn std::error::Error>> {
    let space = atespace("tenant-a")?;
    let actor = ActorName::for_named_lease(&space, "authority-build");
    let mut entry = claim(&space, &actor, &LeaseId::new("same-lease"));
    entry.owner = Some(ActorOwner {
        worker_id: WorkerId::new(worker)?,
        claim_id: ActorClaimId::generate(),
        actor_uid: Some(ActorUid::try_from(UID.to_string())?),
    });
    Ok(entry)
}

struct BareIndex;

#[async_trait::async_trait]
impl ActorIndex for BareIndex {
    async fn put(&self, _entry: &ActorEntry) -> ActorIndexResult<()> {
        Ok(())
    }

    async fn get(&self, _atespace: &str, _actor: &str) -> ActorIndexResult<Option<ActorEntry>> {
        Ok(None)
    }

    async fn delete(&self, _atespace: &str, _actor: &str) -> ActorIndexResult<()> {
        Ok(())
    }

    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn memory_claim_transfer_never_allows_the_old_owner_to_clear_the_new_claim() -> TestResult {
    let index = MemoryActorIndex::default();
    let first = owned("worker-a")?;
    let second = owned("worker-b")?;
    assert!(index.compare_exchange(None, Some(&first)).await?);
    assert!(!index.compare_exchange(None, Some(&second)).await?);
    assert!(!index.compare_exchange(Some(&second), None).await?);
    let mut retained = first.clone();
    retained.retained = true;
    assert!(index.compare_exchange(Some(&first), Some(&retained)).await?);
    assert!(index.compare_exchange(Some(&retained), Some(&second)).await?);
    assert!(!index.compare_exchange(Some(&first), None).await?);
    assert_eq!(index.get(&second.atespace, &second.actor).await?, Some(second.clone()));
    assert!(index.compare_exchange(Some(&second), None).await?);
    assert_eq!(index.get(&second.atespace, &second.actor).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_claim_round_trips_and_is_idempotent_by_actor() -> TestResult {
    let index = MemoryActorIndex::default();
    let space = atespace("tenant-a")?;
    let actor = ActorName::for_named_lease(&space, "build");

    index.put(&claim(&space, &actor, &LeaseId::new("01A"))).await?;
    index.put(&claim(&space, &actor, &LeaseId::new("01B"))).await?;

    let entries = index.all().await?;
    assert_eq!(entries.len(), 1, "a reconnect must not leave a second entry");
    let entry = entries.first().ok_or("missing entry")?;
    assert_eq!(entry.lease_id, LeaseId::new("01B"));
    assert!(entry.named);
    assert!(!entry.retained);
    assert_eq!(entry.actor_name(), Some(actor));
    Ok(())
}

#[tokio::test]
async fn a_named_actors_lease_id_is_recoverable_only_from_the_index() -> TestResult {
    let space = atespace("tenant-a")?;
    let actor = ActorName::for_named_lease(&space, "build");
    assert_eq!(actor.lease_id(), None, "the name deliberately loses it");

    let index = MemoryActorIndex::default();
    let lease_id = LeaseId::new("01ARZ");
    index.put(&claim(&space, &actor, &lease_id)).await?;

    let entry = index.get(space.as_str(), actor.as_str()).await?.ok_or("not indexed")?;
    assert_eq!(entry.lease_id, lease_id);
    Ok(())
}

#[tokio::test]
async fn entries_are_scoped_so_two_tenants_cannot_address_each_others_actors() -> TestResult {
    let a = atespace("tenant-a")?;
    let b = atespace("tenant-b")?;
    let index = MemoryActorIndex::default();
    let lease = LeaseId::new("01ARZ");
    let actor = ActorName::for_lease(&lease)?;
    index.put(&claim(&a, &actor, &lease)).await?;

    assert!(index.get(b.as_str(), actor.as_str()).await?.is_none());
    assert!(index.get(a.as_str(), actor.as_str()).await?.is_some());
    index.delete(b.as_str(), actor.as_str()).await?;
    assert_eq!(index.all().await?.len(), 1);
    index.delete(a.as_str(), actor.as_str()).await?;
    assert!(index.all().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn compare_exchange_requires_one_identity() -> TestResult {
    let index = MemoryActorIndex::default();
    let first = owned("worker-a")?;
    let space = atespace("tenant-b")?;
    let other = claim(&space, &ActorName::for_named_lease(&space, "other"), &LeaseId::new("x"));

    assert!(matches!(
        index.compare_exchange(None, None).await,
        Err(ActorIndexError::ExchangeWithoutIdentity)
    ));
    assert!(matches!(
        index.compare_exchange(Some(&first), Some(&other)).await,
        Err(ActorIndexError::ExchangeIdentitiesDiffer)
    ));
    assert!(matches!(
        exchange_key(None, Some(&other)),
        Ok(key) if key == other.key()
    ));
    Ok(())
}

#[tokio::test]
async fn an_index_without_atomic_claims_refuses_compare_exchange() -> TestResult {
    let index = BareIndex;
    let entry = owned("worker-a")?;
    index.put(&entry).await?;
    assert!(index.get("a", "b").await?.is_none());
    index.delete("a", "b").await?;
    assert!(index.all().await?.is_empty());
    assert!(matches!(
        index.compare_exchange(None, Some(&entry)).await,
        Err(ActorIndexError::AtomicClaimsUnsupported)
    ));
    Ok(())
}

#[test]
fn entries_encode_and_decode_under_their_own_key() -> TestResult {
    let entry = owned("worker-a")?;
    let bytes = encode_entry(&entry)?;
    assert_eq!(decode_entry(&entry.key(), &bytes)?, entry);
    assert!(matches!(
        decode_entry("other.key", &bytes),
        Err(ActorIndexError::EntryKeyMismatch)
    ));
    assert!(matches!(
        decode_entry(&entry.key(), b"not-json"),
        Err(ActorIndexError::InvalidEntry(_))
    ));
    Ok(())
}

#[test]
fn an_entry_without_an_owner_omits_the_field() -> TestResult {
    let space = atespace("tenant-a")?;
    let entry = claim(&space, &ActorName::for_named_lease(&space, "x"), &LeaseId::new("l"));
    let value: serde_json::Value = serde_json::from_slice(&encode_entry(&entry)?)?;
    assert!(value.get("owner").is_none());
    Ok(())
}

#[test]
fn claim_ids_are_ulids() -> TestResult {
    let generated = ActorClaimId::generate();
    assert_eq!(ActorClaimId::try_from(generated.as_str().to_string())?, generated);
    assert_eq!(String::from(generated.clone()), generated.as_str());
    assert_eq!(ActorClaimId::try_from("nope".to_string()), Err(ActorClaimIdError));
    assert_eq!(generated.as_str().len(), 26);
    let lower = generated.as_str().to_lowercase();
    assert_eq!(ActorClaimId::try_from(lower)?, generated);
    assert!(ActorClaimId::try_from("8".repeat(26)).is_err());
    assert!(ActorClaimId::try_from("U".repeat(26)).is_err());
    assert_eq!(
        ActorClaimId::try_from("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string())?.as_str(),
        "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    );
    Ok(())
}

#[test]
fn actor_uids_accept_only_bounded_alphanumerics_and_dashes() -> TestResult {
    let uid = ActorUid::try_from(UID.to_string())?;
    assert_eq!(uid.as_str(), UID);
    assert_eq!(String::from(uid), UID);
    assert_eq!(ActorUid::try_from(String::new()), Err(ActorUidError));
    assert_eq!(ActorUid::try_from("a".repeat(129)), Err(ActorUidError));
    assert_eq!(ActorUid::try_from("has space".to_string()), Err(ActorUidError));
    assert!(ActorUid::try_from("a".repeat(128)).is_ok());
    Ok(())
}

#[test]
fn storage_errors_wrap_their_source() {
    let error = ActorIndexError::storage(std::io::Error::other("boom"));
    assert!(matches!(error, ActorIndexError::Storage(_)));
}

#[test]
fn settle_maps_success_lost_races_and_failures() {
    let ok: Result<u8, std::io::Error> = Ok(1);
    assert!(matches!(settle(ok, |_| false), Ok(true)));
    let lost: Result<u8, std::io::Error> = Err(std::io::Error::other("lost"));
    assert!(matches!(settle(lost, |_| true), Ok(false)));
    let failed: Result<u8, std::io::Error> = Err(std::io::Error::other("failed"));
    assert!(matches!(settle(failed, |_| false), Err(ActorIndexError::Storage(_))));
}

#[test]
fn only_a_wrong_revision_counts_as_a_lost_update_or_delete() {
    assert!(lost_update(&kv::UpdateError::new(
        kv::UpdateErrorKind::WrongLastRevision
    )));
    assert!(!lost_update(&kv::UpdateError::new(kv::UpdateErrorKind::TimedOut)));
    assert!(lost_delete(&kv::DeleteError::new(
        kv::DeleteErrorKind::WrongLastRevision
    )));
    assert!(!lost_delete(&kv::DeleteError::new(kv::DeleteErrorKind::TimedOut)));
}
