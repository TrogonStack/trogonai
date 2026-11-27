use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Mutex;
use trogon_sandbox_protocol::{AuthorityHost, Provider, PurgeOutcome};

use super::{ORIGINAL_UID, first_lease, second_lease, serve_agent, tenant_atespace, worker_behind};
use crate::actor_index::ActorIndex;
use crate::actor_index::memory::MemoryActorIndex;
use crate::adapter::tests_support::{FakeControl, TestResult, lease_request};
use crate::agent::ActorTarget;
use crate::ate_client::{AteError, Control};
use crate::naming::ActorName;
use crate::proto;

/// A worker creates an actor, its reply is lost, and the caller retries on another worker.
async fn create_retry(retained: bool) -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let lease_id = first_lease();
    let atespace = tenant_atespace()?;
    let actor = if retained {
        ActorName::for_named_lease(&atespace, "retained-connect")
    } else {
        ActorName::for_lease(&lease_id)?
    };
    let binds = Arc::new(AtomicUsize::new(0));
    let agent = serve_agent(
        ActorTarget::new(atespace.clone(), actor.clone(), 8110),
        binds.clone(),
        Arc::new(Mutex::new(None)),
    )
    .await?;
    let control = Arc::new(FakeControl::running().with_uid(ORIGINAL_UID).with_transitions());
    let first = worker_behind(&agent, control.clone(), index.clone(), "actual-substrate-a")?;
    let second = worker_behind(&agent, control.clone(), index.clone(), "actual-substrate-b")?;
    let request = lease_request(retained.then_some("retained-connect"), retained)?;

    // The first worker allocates and binds the actor. The grant is dropped, as a lost reply is.
    let original = Provider::lease(&first, &request, &lease_id).await?;
    let original_held = AuthorityHost::held_leases(&first).await?;
    assert_eq!(binds.load(Ordering::SeqCst), 1);
    assert!(first.active_lease(&lease_id).await.is_some());

    let mut retry_id = lease_id.clone();
    if retained {
        Provider::cancel(&first, &lease_id).await?;
        assert!(first.active_lease(&lease_id).await.is_none());
        assert_eq!(control.delete_count(), 0);
        retry_id = second_lease();
    }
    let original_claim = index.get(atespace.as_str(), actor.as_str()).await?;
    control.fail_create_with(AteError::AlreadyExists {
        message: "the original actor still exists".into(),
    });

    let reply = Provider::lease(&second, &request, &retry_id).await;

    let second_held = second.active_lease(&retry_id).await.is_some();
    let final_claim = index.get(atespace.as_str(), actor.as_str()).await?;
    let bind_count = binds.load(Ordering::SeqCst);
    let first_owner = original_claim
        .as_ref()
        .and_then(|entry| entry.owner.as_ref())
        .ok_or("the original claim has no owner")?;
    assert_eq!(first_owner.worker_id, *first.worker_id());
    assert_eq!(
        first_owner.actor_uid.as_ref().map(|uid| uid.as_str()),
        Some(ORIGINAL_UID)
    );
    assert_eq!(original.worker_id.as_ref(), Some(first.worker_id()));

    if retained {
        let effects = (control.suspend_count(), control.delete_count());
        let stale = AuthorityHost::purge(&first, original_held.first().ok_or("no held lease")?).await?;
        assert_eq!(stale, PurgeOutcome::Stale);
        assert_eq!(effects, (control.suspend_count(), control.delete_count()));
    }
    control.set_statuses(vec![proto::ActorState::Suspended]);
    Provider::cancel(&first, &lease_id).await?;
    if retained {
        Provider::purge(&second, &retry_id).await?;
    } else {
        Provider::cancel(&second, &lease_id).await?;
    }
    assert!(first.active_lease(&lease_id).await.is_none());
    assert!(second.active_lease(&retry_id).await.is_none());
    let remaining = control
        .get_actor(proto::ObjectRef {
            atespace: atespace.as_str().into(),
            name: actor.as_str().into(),
        })
        .await;
    assert!(matches!(remaining, Err(AteError::NotFound { .. })));

    if retained {
        reply?;
        let final_claim = final_claim.ok_or("the transferred actor lost its ownership")?;
        assert!(original_claim.as_ref().is_some_and(|entry| entry.retained));
        assert_eq!(final_claim.lease_id, retry_id);
        assert!(!final_claim.retained);
        let final_owner = final_claim.owner.as_ref().ok_or("the transferred claim has no owner")?;
        assert_eq!(final_owner.worker_id, *second.worker_id());
        assert_eq!(
            final_owner.actor_uid.as_ref().map(|uid| uid.as_str()),
            Some(ORIGINAL_UID)
        );
        assert_ne!(final_owner.claim_id, first_owner.claim_id);
        assert!(second_held);
        assert_eq!(bind_count, 2);
    } else {
        assert!(reply.is_err());
        assert_eq!(original_claim, final_claim);
        assert!(!second_held);
        assert_eq!(bind_count, 1);
    }
    Ok(())
}

#[tokio::test]
async fn authority_lost_create_reply_never_rebinds_the_same_actor_on_a_second_replica() -> TestResult {
    create_retry(false).await
}

#[tokio::test]
async fn authority_retained_named_connect_transfers_the_exact_parked_actor_between_replicas() -> TestResult {
    create_retry(true).await
}
