use std::error::Error;

use super::keys::parse_active_key;
use super::*;
use crate::test_support::LocalNats;
use std::sync::Arc;
use std::time::SystemTime;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, OperationClaim, OperationId, OperationRequest, OperationResult, OperationState, TenantId,
};

type TestResult = Result<(), Box<dyn Error>>;

fn tenant(name: &str) -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new(name)?)
}
fn id(name: &str) -> Result<OperationId, Box<dyn Error>> {
    Ok(OperationId::new(name)?)
}
fn lease(name: &str) -> LeaseId {
    LeaseId::new(name)
}
fn pause() -> OperationRequest {
    OperationRequest::Pause {
        max_paused_seconds: None,
    }
}
fn ledger(owner: &str) -> Operations {
    Operations::new(Arc::new(InMemoryOperationStore::new()), owner)
}
fn shared(store: &Arc<dyn OperationStore>, owner: &str) -> Operations {
    Operations::new(Arc::clone(store), owner)
}
fn paused() -> OperationResult {
    OperationResult::Paused {
        reap_at: SystemTime::UNIX_EPOCH,
    }
}

#[test]
fn keys_do_not_collide_across_tenants_or_characters() -> TestResult {
    // The old `{tenant}_{name}` scheme maps both of these to the same key.
    let a = op_key(&tenant("a:b")?, &id("x")?);
    let b = op_key(&tenant("a_b")?, &id("x")?);
    assert_ne!(a, b);
    assert_ne!(
        active_key(&tenant("a")?, &id("b")?),
        active_key(&tenant("a.b")?, &id("b")?)
    );
    assert_ne!(target_key(&lease("a.b")), target_key(&lease("a_b")));
    let key = active_key(&tenant("a:b")?, &id("op-1")?);
    assert_eq!(parse_active_key(&key), Some((tenant("a:b")?, id("op-1")?)));
    Ok(())
}

#[test]
fn fingerprint_ignores_the_id_but_not_the_request() -> TestResult {
    let base = fingerprint(&tenant("t")?, &lease("l"), &pause())?;
    assert_eq!(base, fingerprint(&tenant("t")?, &lease("l"), &pause())?);
    assert_ne!(
        base,
        fingerprint(
            &tenant("t")?,
            &lease("l"),
            &OperationRequest::Pause {
                max_paused_seconds: Some(5)
            }
        )?
    );
    assert_ne!(base, fingerprint(&tenant("t")?, &lease("m"), &pause())?);
    assert_ne!(base, fingerprint(&tenant("u")?, &lease("l"), &pause())?);
    assert_ne!(
        base,
        fingerprint(&tenant("t")?, &lease("l"), &OperationRequest::Resume)?
    );
    Ok(())
}

#[tokio::test]
async fn begin_claim_finish_walks_queued_running_succeeded() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    let Begun::Created(created) = ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await? else {
        return Err("expected a new operation".into());
    };
    assert_eq!(created.record.state, OperationState::Queued);
    assert_eq!(ops.list_active().await?.len(), 1);

    let (running, token) = ops.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    assert_eq!(running.state, OperationState::Running);
    assert_eq!(token.generation, 1);
    assert_eq!(running.attempts, 1);

    let Settled::Settled(done) = ops.finish(&token, Ok(paused()), now).await? else {
        return Err("claim should still hold".into());
    };
    assert_eq!(done.state, OperationState::Succeeded);
    assert_eq!(done.result, Some(paused()));
    assert!(done.gc_after.is_some() && done.claim.is_none());
    assert!(ops.list_active().await?.is_empty());
    // Terminal states are immutable: nothing can claim or finish again.
    assert!(ops.claim(&tenant("t")?, &id("op")?, now).await?.is_none());
    assert!(matches!(ops.finish(&token, Ok(paused()), now).await?, Settled::Fenced));
    Ok(())
}

#[tokio::test]
async fn a_failure_is_stored_as_failed_with_its_error() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?;
    let (_, token) = ops.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    let error = LeaseError::LeaseNotRunning { lease_id: lease("l") };
    let Settled::Settled(done) = ops.finish(&token, Err(error.clone()), now).await? else {
        return Err("settled".into());
    };
    assert_eq!(done.state, OperationState::Failed);
    assert_eq!(done.error, Some(error));
    Ok(())
}

#[tokio::test]
async fn the_same_id_with_the_same_request_replays_and_with_another_conflicts() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?;
    assert!(matches!(
        ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?,
        Begun::Existing(_)
    ));
    for (lease_id, request) in [
        ("l", OperationRequest::Resume),
        ("other", pause()),
        (
            "l",
            OperationRequest::Pause {
                max_paused_seconds: Some(1),
            },
        ),
    ] {
        let Begun::Conflict(LeaseError::OperationConflict { operation_id, .. }) = ops
            .begin(&tenant("t")?, &id("op")?, &lease(lease_id), request, now)
            .await?
        else {
            return Err("expected a conflict".into());
        };
        assert_eq!(operation_id, id("op")?);
    }
    // Another tenant using the same id is a different operation, not a conflict.
    assert!(matches!(
        ops.begin(&tenant("u")?, &id("op")?, &lease("l2"), pause(), now).await?,
        Begun::Created(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_holds_one_live_operation_and_frees_on_finish() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("first")?, &lease("l"), pause(), now)
        .await?;
    assert!(matches!(
        ops.begin(
            &tenant("t")?,
            &id("second")?,
            &lease("l"),
            OperationRequest::Resume,
            now
        )
        .await?,
        Begun::Conflict(LeaseError::OperationConflict { .. })
    ));
    let (_, token) = ops.claim(&tenant("t")?, &id("first")?, now).await?.ok_or("absent")?;
    ops.finish(&token, Ok(paused()), now).await?;
    assert!(matches!(
        ops.begin(
            &tenant("t")?,
            &id("second")?,
            &lease("l"),
            OperationRequest::Resume,
            now
        )
        .await?,
        Begun::Created(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_marker_left_by_a_finished_operation_does_not_block_the_lease() -> TestResult {
    let store: Arc<dyn OperationStore> = Arc::new(InMemoryOperationStore::new());
    let ops = shared(&store, "me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("first")?, &lease("l"), pause(), now)
        .await?;
    let (_, token) = ops.claim(&tenant("t")?, &id("first")?, now).await?.ok_or("absent")?;
    ops.finish(&token, Ok(paused()), now).await?;
    // Simulate a crash between the terminal write and the marker cleanup.
    store
        .create(
            &target_key(&lease("l")),
            super::codec::encode(&(tenant("t")?, id("first")?))?,
        )
        .await?;
    assert!(matches!(
        ops.begin(&tenant("t")?, &id("next")?, &lease("l"), OperationRequest::Resume, now)
            .await?,
        Begun::Created(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_stale_owner_cannot_write_after_a_takeover() -> TestResult {
    let store: Arc<dyn OperationStore> = Arc::new(InMemoryOperationStore::new());
    let old = shared(&store, "old");
    let new = shared(&store, "new");
    let now = SystemTime::now();
    old.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?;
    let (_, stale) = old.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    let (taken, fresh) = new.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    assert_eq!(fresh.generation, stale.generation + 1);
    assert_eq!(taken.claim.as_ref().ok_or("absent")?.owner, "new");

    assert!(matches!(old.finish(&stale, Ok(paused()), now).await?, Settled::Fenced));
    let still = new.get(&tenant("t")?, &id("op")?).await?.ok_or("absent")?;
    assert_eq!(still.record.state, OperationState::Running);
    assert!(matches!(
        new.finish(&fresh, Ok(paused()), now).await?,
        Settled::Settled(_)
    ));
    Ok(())
}

#[tokio::test]
async fn the_same_owner_with_an_old_generation_is_fenced_too() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?;
    let (_, first) = ops.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    let (_, second) = ops.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    assert_eq!((first.generation, second.generation), (1, 2));
    assert!(matches!(ops.finish(&first, Ok(paused()), now).await?, Settled::Fenced));
    Ok(())
}

#[tokio::test]
async fn reclaimability_follows_state_owner_and_age() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?;
    let queued = ops.get(&tenant("t")?, &id("op")?).await?.ok_or("absent")?.record;
    assert!(ops.is_reclaimable(&queued, now, false, false));

    let mut running = queued.clone();
    running.state = OperationState::Running;
    running.claim = Some(OperationClaim {
        owner: "me".into(),
        generation: 1,
        renewed_at: now,
    });
    assert!(!ops.is_reclaimable(&running, now, true, true));
    assert!(ops.is_reclaimable(&running, now, false, false));

    running.claim.as_mut().ok_or("absent")?.owner = "other".into();
    assert!(!ops.is_reclaimable(&running, now, false, false));
    assert!(ops.is_reclaimable(&running, now, true, false));
    assert!(ops.is_reclaimable(&running, now + STALE_CLAIM_AFTER, false, false));

    running.state = OperationState::Succeeded;
    assert!(!ops.is_reclaimable(&running, now, true, false));
    Ok(())
}

#[tokio::test]
async fn a_requeued_operation_waits_for_its_retry_time_and_fences_the_old_claim() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), now).await?;
    let (_, first) = ops.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    let reason = LeaseError::Backend {
        message: "worker unreachable".into(),
    };
    let retry_at = now + RETRY_AFTER;
    let Settled::Settled(queued) = ops.requeue(&first, &reason, retry_at).await? else {
        return Err("requeued".into());
    };
    assert_eq!(queued.state, OperationState::Queued);
    assert!(!ops.is_reclaimable(&queued, now, true, false));
    assert!(ops.is_reclaimable(&queued, retry_at, false, false));
    assert!(matches!(ops.finish(&first, Ok(paused()), now).await?, Settled::Fenced));
    let (again, second) = ops.claim(&tenant("t")?, &id("op")?, retry_at).await?.ok_or("absent")?;
    assert_eq!((second.generation, again.attempts), (2, 2));
    Ok(())
}

#[tokio::test]
async fn another_tenant_cannot_read_an_operation() -> TestResult {
    let ops = ledger("me");
    ops.begin(&tenant("t")?, &id("op")?, &lease("l"), pause(), SystemTime::now())
        .await?;
    assert!(ops.get(&tenant("t")?, &id("op")?).await?.is_some());
    assert!(ops.get(&tenant("other")?, &id("op")?).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn nats_kv_store_races_resolve_through_revisions() -> TestResult {
    let nats = LocalNats::start().await;
    let js = async_nats::jetstream::new(nats.client.clone());
    let a: Arc<dyn OperationStore> = Arc::new(NatsKvOperationStore::new(js.clone(), OPERATIONS_BUCKET, 1).await?);
    let b: Arc<dyn OperationStore> = Arc::new(NatsKvOperationStore::new(js, OPERATIONS_BUCKET, 1).await?);
    assert!(a.keys("active.").await?.is_empty());

    let first = shared(&a, "a");
    let second = shared(&b, "b");
    let now = SystemTime::now();
    let (t, op, l) = (tenant("t")?, id("op")?, lease("l"));
    let (x, y) = tokio::join!(
        first.begin(&t, &op, &l, pause(), now),
        second.begin(&t, &op, &l, pause(), now),
    );
    let created = [x?, y?].iter().filter(|b| matches!(b, Begun::Created(_))).count();
    assert_eq!(created, 1, "exactly one writer creates the operation");
    assert_eq!(first.list_active().await?.len(), 1);

    // Two owners race to claim: both may win in turn, but generations never repeat.
    let (c, d) = tokio::join!(first.claim(&t, &op, now), second.claim(&t, &op, now));
    let (c, d) = (c?, d?);
    assert!(c.is_some() || d.is_some());
    if let (Some((_, t1)), Some((_, t2))) = (&c, &d) {
        assert_ne!(t1.generation, t2.generation);
    }
    let (_, loser_or_winner) = c.or(d).ok_or("absent")?;
    let current = first.get(&tenant("t")?, &id("op")?).await?.ok_or("absent")?;
    let owner = current.record.claim.as_ref().ok_or("absent")?.owner.clone();
    let (holder, other) = if owner == "a" {
        (&first, &second)
    } else {
        (&second, &first)
    };
    let live = ClaimToken {
        generation: current.record.claim.as_ref().ok_or("absent")?.generation,
        ..loser_or_winner
    };
    assert!(matches!(other.finish(&live, Ok(paused()), now).await?, Settled::Fenced));
    assert!(matches!(
        holder.finish(&live, Ok(paused()), now).await?,
        Settled::Settled(_)
    ));
    assert!(first.list_active().await?.is_empty());
    assert!(a.get(&target_key(&lease("l"))).await?.is_none());
    Ok(())
}

fn create_request(lease_id: &str, attempt: &str) -> Result<(LeaseId, OperationRequest), Box<dyn Error>> {
    let request: trogon_sandbox_contract::LeaseRequest = serde_json::from_value(serde_json::json!({
        "tenant": "t",
        "provider_pref": ["mock"],
        "spec": {"image":{"kind":"registry","reference":{"registry":"","repository":"img"}},"workspace":{"empty":{}},"cpu_seconds":1,"memory_mib":64},
        "ttl_seconds": 60,
        "idempotency_key": "k"
    }))?;
    Ok((
        lease(lease_id),
        OperationRequest::Create {
            request: Box::new(request),
            attempt_id: trogon_sandbox_contract::DispatchAttemptId::new(attempt)?,
            dispatch: None,
            quota: None,
        },
    ))
}

#[test]
fn a_create_is_identified_by_its_ask_not_by_the_ids_the_broker_chose() -> TestResult {
    let (first_lease, first) = create_request("lease-a", "attempt-a")?;
    let (second_lease, second) = create_request("lease-b", "attempt-b")?;
    assert_eq!(
        fingerprint(&tenant("t")?, &first_lease, &first)?,
        fingerprint(&tenant("t")?, &second_lease, &second)?
    );
    assert_ne!(
        fingerprint(&tenant("t")?, &first_lease, &first)?,
        fingerprint(&tenant("u")?, &first_lease, &first)?
    );
    Ok(())
}

#[tokio::test]
async fn save_dispatch_needs_the_claim_and_a_create() -> TestResult {
    let ops = ledger("me");
    let now = SystemTime::now();
    let (lease_id, request) = create_request("lease-a", "attempt-a")?;
    ops.begin(&tenant("t")?, &id("op")?, &lease_id, request, now).await?;
    let payload = trogon_sandbox_contract::ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: match ops
            .get(&tenant("t")?, &id("op")?)
            .await?
            .ok_or("absent")?
            .record
            .request
        {
            Some(OperationRequest::Create { request, .. }) => *request,
            _ => return Err("a create".into()),
        },
        correlation_id: None,
        resolved_drives: vec![],
        target: None,
    };
    let (_, token) = ops.claim(&tenant("t")?, &id("op")?, now).await?.ok_or("absent")?;
    let stale = ClaimToken {
        generation: token.generation + 1,
        ..token.clone()
    };
    assert!(!ops.save_dispatch(&stale, &payload).await?);
    assert!(ops.save_dispatch(&token, &payload).await?);
    let stored = ops.get(&tenant("t")?, &id("op")?).await?.ok_or("absent")?;
    let Some(OperationRequest::Create { dispatch, .. }) = stored.record.request else {
        return Err("a create".into());
    };
    assert_eq!(dispatch.as_deref(), Some(&payload));

    // A finished create keeps no request, so its environment is not retained.
    let Settled::Settled(done) = ops
        .finish(&token, Err(LeaseError::Backend { message: "x".into() }), now)
        .await?
    else {
        return Err("claim holds".into());
    };
    assert!(done.request.is_none());
    assert!(!ops.save_dispatch(&token, &payload).await?);
    Ok(())
}
