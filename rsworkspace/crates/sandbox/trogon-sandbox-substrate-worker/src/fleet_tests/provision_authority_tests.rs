use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    LeaseRequest, OwnedLease, ProviderKind, ProviderLease, WorkerAuthorityRequest, WorkerAuthorityResponse,
    WorkerHeartbeat,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityHost, AuthorityMode, Provider, ProviderError, PurgeOutcome, subjects,
};

use super::{Gate, GateControls, ORIGINAL_UID, first_lease, serve_agent, tenant_atespace, worker_behind};
use crate::actor_index::ActorIndex;
use crate::actor_index::memory::MemoryActorIndex;
use crate::adapter::SubstrateProvider;
use crate::adapter::tests_support::{FakeControl, TestResult, lease_request};
use crate::agent::{ActorAgent, ActorTarget};
use crate::ate_client::{AteError, Control};
use crate::naming::ActorName;
use crate::{proto, worker};

struct Fixture {
    server: JetStreamTestServer,
    bind_gate: Arc<Mutex<Option<Gate>>>,
    control: Arc<FakeControl>,
    index: Arc<MemoryActorIndex>,
    first: Arc<SubstrateProvider>,
    request: LeaseRequest,
    actor: proto::ObjectRef,
    binds: Arc<AtomicUsize>,
    _agent: ActorAgent,
}

async fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let index = Arc::new(MemoryActorIndex::default());
    let atespace = tenant_atespace()?;
    let actor = ActorName::for_named_lease(&atespace, "custody-failure");
    let binds = Arc::new(AtomicUsize::new(0));
    let bind_gate: Arc<Mutex<Option<Gate>>> = Arc::new(Mutex::new(None));
    let agent = serve_agent(
        ActorTarget::new(atespace.clone(), actor.clone(), 8110),
        binds.clone(),
        bind_gate.clone(),
    )
    .await?;
    let control = Arc::new(FakeControl::running().with_uid(ORIGINAL_UID).with_transitions());
    let first = Arc::new(worker_behind(&agent, control.clone(), index.clone(), "custody-first")?);
    Ok(Fixture {
        server,
        bind_gate,
        control,
        index,
        first,
        request: lease_request(Some("custody-failure"), true)?,
        actor: proto::ObjectRef {
            atespace: atespace.as_str().into(),
            name: actor.as_str().into(),
        },
        binds,
        _agent: agent,
    })
}

/// Starts a lease and leaves it blocked while the guest bind is still in flight.
async fn lease_blocked_at_bind(
    fixture: &Fixture,
) -> Result<
    (
        tokio::task::JoinHandle<Result<ProviderLease, ProviderError>>,
        GateControls,
    ),
    Box<dyn Error>,
> {
    let (gate, controls) = Gate::new();
    *fixture.bind_gate.lock().await = Some(gate);
    let allocation = tokio::spawn({
        let provider = fixture.first.clone();
        let request = fixture.request.clone();
        async move { Provider::lease(provider.as_ref(), &request, &first_lease()).await }
    });
    Ok((allocation, controls))
}

fn authority_config() -> AuthorityConfig {
    let mut authority = AuthorityConfig::new(AuthorityMode::Enforce);
    authority.interval = Duration::from_millis(10);
    authority.min_lease_age = Duration::ZERO;
    authority.confirmations = 1;
    authority.grace = Duration::ZERO;
    authority
}

async fn assert_cleaned_up(fixture: &Fixture) -> TestResult {
    assert!(fixture.index.all().await?.is_empty());
    assert!(matches!(
        fixture.control.get_actor(fixture.actor.clone()).await,
        Err(AteError::NotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn authority_inflight_provision_is_not_reclaimed_from_a_pregrant_broker_reply() -> TestResult {
    let fixture = fixture().await?;
    let client = fixture.server.client().await;
    let context = fixture.server.jetstream().await;
    let lease_id = first_lease();
    let (allocation, controls) = lease_blocked_at_bind(&fixture).await?;
    tokio::time::timeout(Duration::from_secs(5), controls.entered).await??;
    let claim = fixture
        .index
        .get(&fixture.actor.atespace, &fixture.actor.name)
        .await?
        .ok_or("the allocation has no claim")?;
    let recorded = claim.owner.and_then(|owner| owner.actor_uid);
    assert_eq!(recorded.as_ref().map(|uid| uid.as_str()), Some(ORIGINAL_UID));
    assert!(fixture.first.active_lease(&lease_id).await.is_some());
    assert_eq!(fixture.binds.load(Ordering::SeqCst), 1);
    let inflight_inventory = AuthorityHost::held_leases(fixture.first.as_ref()).await?;
    let owner = fixture.first.worker_id().clone();
    let mut queries = client.subscribe(subjects::worker_authority_check(&owner)).await?;
    let mut heartbeats = client
        .subscribe(subjects::worker_heartbeat(ProviderKind::Substrate, &owner))
        .await?;
    client.flush().await?;
    let broker_owned = Arc::new(AtomicBool::new(false));
    let pregrant_checks = Arc::new(AtomicUsize::new(0));
    let owned_checks = Arc::new(AtomicUsize::new(0));
    let oracle = tokio::spawn({
        let client = client.clone();
        let broker_owned = broker_owned.clone();
        let pregrant_checks = pregrant_checks.clone();
        let owned_checks = owned_checks.clone();
        async move {
            while let Some(message) = queries.next().await {
                let Ok(request) = serde_json::from_slice::<WorkerAuthorityRequest>(&message.payload) else {
                    continue;
                };
                let response = if broker_owned.load(Ordering::SeqCst) {
                    owned_checks.fetch_add(1, Ordering::SeqCst);
                    WorkerAuthorityResponse {
                        owned: request
                            .lease_ids
                            .into_iter()
                            .map(|lease_id| OwnedLease { lease_id, epoch: None })
                            .collect(),
                        not_assigned: Vec::new(),
                    }
                } else {
                    pregrant_checks.fetch_add(1, Ordering::SeqCst);
                    WorkerAuthorityResponse {
                        owned: Vec::new(),
                        not_assigned: request.lease_ids,
                    }
                };
                let (Some(reply), Ok(payload)) = (message.reply, serde_json::to_vec(&response)) else {
                    continue;
                };
                let _ = client.publish(reply, payload.into()).await;
            }
        }
    });
    let cancel = CancellationToken::new();
    worker::spawn_heartbeat_task(client.clone(), fixture.first.clone(), cancel.clone());
    let mut running = tokio::spawn(worker::run_with_authority(
        client.clone(),
        context,
        fixture.first.clone(),
        owner.clone(),
        cancel.clone(),
        authority_config(),
    ));

    let heartbeat = tokio::time::timeout(Duration::from_secs(5), heartbeats.next())
        .await?
        .ok_or("provision heartbeat stream closed")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    assert_eq!(heartbeat.worker_id, owner);
    assert_eq!(heartbeat.capacity.current_leases, 1);
    assert!(!running.is_finished());
    tokio::time::sleep(Duration::from_millis(50)).await;
    broker_owned.store(true, Ordering::SeqCst);
    let _ = controls.proceed.send(());
    let grant = tokio::time::timeout(Duration::from_secs(5), allocation).await??;
    let observation = tokio::time::timeout(Duration::from_secs(5), async {
        while owned_checks.load(Ordering::SeqCst) < 2 && fixture.control.delete_count() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let survived = fixture.first.active_lease(&lease_id).await.is_some()
        && fixture.control.get_actor(fixture.actor.clone()).await.is_ok()
        && fixture.control.delete_count() == 0;

    cancel.cancel();
    if tokio::time::timeout(Duration::from_secs(5), &mut running)
        .await
        .is_err()
    {
        running.abort();
        let _ = running.await;
    }
    oracle.abort();
    let _ = oracle.await;
    Provider::purge(fixture.first.as_ref(), &lease_id).await?;
    assert_cleaned_up(&fixture).await?;
    assert!(inflight_inventory.is_empty());
    assert_eq!(pregrant_checks.load(Ordering::SeqCst), 0);
    assert_eq!(grant?.worker_id.as_ref(), Some(&owner));
    assert!(observation.is_ok());
    assert!(owned_checks.load(Ordering::SeqCst) >= 2);
    assert!(survived);
    Ok(())
}

#[tokio::test]
async fn authority_abandoned_guest_bind_keeps_its_exact_allocation_visible_for_cleanup() -> TestResult {
    let fixture = fixture().await?;
    let lease_id = first_lease();
    let (allocation, controls) = lease_blocked_at_bind(&fixture).await?;
    tokio::time::timeout(Duration::from_secs(5), controls.entered).await??;
    let claim = fixture
        .index
        .get(&fixture.actor.atespace, &fixture.actor.name)
        .await?
        .ok_or("the allocation has no claim")?;
    assert_eq!(
        claim
            .owner
            .and_then(|owner| owner.actor_uid)
            .as_ref()
            .map(|uid| uid.as_str()),
        Some(ORIGINAL_UID)
    );
    allocation.abort();
    assert!(allocation.await.is_err_and(|error| error.is_cancelled()));
    let _ = controls.proceed.send(());

    let held = AuthorityHost::held_leases(fixture.first.as_ref()).await?;

    assert_eq!(held.len(), 1);
    let held = held.first().ok_or("no held lease")?;
    assert_eq!(held.lease_id(), &lease_id);
    assert_eq!(
        AuthorityHost::purge(fixture.first.as_ref(), held).await?,
        PurgeOutcome::Purged
    );
    assert!(fixture.first.agent_for(&lease_id).await.is_none());
    assert_cleaned_up(&fixture).await?;
    Provider::purge(fixture.first.as_ref(), &lease_id).await?;
    assert_cleaned_up(&fixture).await
}
