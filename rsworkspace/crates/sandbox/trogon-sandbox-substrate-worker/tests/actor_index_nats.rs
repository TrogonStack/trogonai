//! JetStream-backed actor index behavior. Needs a container runtime for the NATS server.

use std::sync::Arc;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};
use trogon_sandbox_substrate_worker::{
    ACTOR_KV_BUCKET, ActorClaimId, ActorEntry, ActorIndex, ActorName, ActorOwner, ActorUid, Atespace, NatsActorIndex,
    claim,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn owned(worker: &str) -> Result<ActorEntry, Box<dyn std::error::Error>> {
    let space = Atespace::for_tenant(&TenantId::new("tenant-a")?)?;
    let actor = ActorName::for_named_lease(&space, "authority-build");
    let mut entry = claim(&space, &actor, &LeaseId::new("same-lease"));
    entry.owner = Some(ActorOwner {
        worker_id: WorkerId::new(worker)?,
        claim_id: ActorClaimId::generate(),
        actor_uid: Some(ActorUid::try_from("00000000-0000-4000-8000-000000000001".to_string())?),
    });
    Ok(entry)
}

async fn assert_claim_transfer(index: &dyn ActorIndex) -> TestResult {
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
    assert_eq!(index.get(&second.atespace, &second.actor).await?, Some(second));
    Ok(())
}

#[tokio::test]
async fn competing_and_reopened_claims_keep_one_owner_for_the_same_actor() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let first = Arc::new(NatsActorIndex::new(context.clone()).await?);
    let second = Arc::new(NatsActorIndex::new(context.clone()).await?);
    let a = owned("worker-a")?;
    let b = owned("worker-b")?;
    let (left, right) = tokio::join!(
        first.compare_exchange(None, Some(&a)),
        second.compare_exchange(None, Some(&b))
    );
    assert_ne!(left?, right?);
    let winner = first.get(&a.atespace, &a.actor).await?.ok_or("no winning claim")?;
    drop(first);
    drop(second);
    let reopened = NatsActorIndex::new(context).await?;
    assert!(!reopened.compare_exchange(None, Some(&a)).await?);
    assert!(!reopened.compare_exchange(None, Some(&b)).await?);
    assert_eq!(reopened.get(&a.atespace, &a.actor).await?, Some(winner.clone()));
    let loser = if winner == a { b } else { a };
    assert!(!reopened.compare_exchange(Some(&loser), None).await?);
    assert!(reopened.compare_exchange(Some(&winner), None).await?);
    assert!(reopened.get(&winner.atespace, &winner.actor).await?.is_none());
    assert_claim_transfer(&reopened).await
}

#[tokio::test]
async fn entries_put_listed_and_deleted_through_jetstream() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let index = NatsActorIndex::new(server.jetstream().await).await?;
    let entry = owned("worker-a")?;
    assert!(index.all().await?.is_empty());
    index.put(&entry).await?;
    assert_eq!(index.all().await?, vec![entry.clone()]);
    index.delete(&entry.atespace, &entry.actor).await?;
    assert!(index.get(&entry.atespace, &entry.actor).await?.is_none());
    assert!(index.all().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn malformed_actor_ownership_is_not_treated_as_an_unclaimed_actor() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let index = NatsActorIndex::new(context.clone()).await?;
    let kv = context.get_key_value(ACTOR_KV_BUCKET).await?;
    let entry = owned("worker-a")?;
    kv.put(entry.key(), bytes::Bytes::from_static(b"not-json")).await?;
    assert!(index.get(&entry.atespace, &entry.actor).await.is_err());
    assert!(index.all().await.is_err());
    assert!(index.compare_exchange(Some(&entry), None).await.is_err());
    Ok(())
}
