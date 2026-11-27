//! Several workers sharing one ownership index and one control plane.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use tokio::sync::{Mutex, oneshot};
use trogon_sandbox_actor_agent::{BIND_PATH, READYZ_PATH};
use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};

use crate::actor_index::memory::MemoryActorIndex;
use crate::actor_index::{ActorClaimId, ActorEntry, ActorIndex, ActorOwner, ActorUid};
use crate::adapter::SubstrateProvider;
use crate::adapter::tests_support::{FakeControl, TestResult, config, provider_over, provider_with};
use crate::agent::tests::{agent_router, serve};
use crate::agent::{ActorAgent, ActorTarget};
use crate::ate_client::{AteError, Control};
use crate::naming::{ActorName, Atespace};
use crate::{proto, recovery};

mod create_tests;
mod custody_failure_tests;
mod guard_reaper_tests;
mod provision_authority_tests;

const ORIGINAL_UID: &str = "00000000-0000-4000-8000-000000000001";
const REPLACEMENT_UID: &str = "00000000-0000-4000-8000-000000000002";

/// The first lease the fleet tests hand out.
fn first_lease() -> LeaseId {
    LeaseId::new("01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

/// The second lease the fleet tests hand out.
fn second_lease() -> LeaseId {
    LeaseId::new("01ARZ3NDEKTSV4RRFFQ69G5FAW")
}

fn tenant_atespace() -> Result<Atespace, Box<dyn Error>> {
    Ok(Atespace::for_tenant(&TenantId::new("tenant-a")?)?)
}

/// Holds a guest bind until the test lets it go.
struct Gate {
    entered: oneshot::Sender<()>,
    proceed: oneshot::Receiver<()>,
}

/// The test's side of a [`Gate`]: it learns when the call arrives and decides when it goes on.
struct GateControls {
    entered: oneshot::Receiver<()>,
    proceed: oneshot::Sender<()>,
}

impl Gate {
    fn new() -> (Self, GateControls) {
        let (entered, at_gate) = oneshot::channel();
        let (proceed, resume) = oneshot::channel();
        (
            Self {
                entered,
                proceed: resume,
            },
            GateControls {
                entered: at_gate,
                proceed,
            },
        )
    }
}

type SharedGate = Arc<Mutex<Option<Gate>>>;

/// A guest agent behind `target` that counts the binds it accepts and holds the next one at `gate`.
async fn serve_agent(
    target: ActorTarget,
    binds: Arc<AtomicUsize>,
    gate: SharedGate,
) -> Result<ActorAgent, Box<dyn Error>> {
    let router = Router::new()
        .route(READYZ_PATH, get(|| async { StatusCode::OK }))
        .route(
            BIND_PATH,
            post(move || {
                let binds = binds.clone();
                let gate = gate.clone();
                async move {
                    binds.fetch_add(1, Ordering::SeqCst);
                    let pending = gate.lock().await.take();
                    if let Some(pending) = pending {
                        let _ = pending.entered.send(());
                        if pending.proceed.await.is_err() {
                            return StatusCode::SERVICE_UNAVAILABLE;
                        }
                    }
                    StatusCode::NO_CONTENT
                }
            }),
        );
    serve(target, router).await
}

/// A worker named `worker` over a control plane and an index it shares with other workers.
fn worker(
    control: Arc<dyn Control>,
    index: Arc<dyn ActorIndex>,
    worker: &str,
) -> Result<SubstrateProvider, Box<dyn Error>> {
    Ok(provider_with(control, index)?.with_worker_id(WorkerId::new(worker)?))
}

/// A worker whose router fronts `agent`.
fn worker_behind(
    agent: &ActorAgent,
    control: Arc<dyn Control>,
    index: Arc<dyn ActorIndex>,
    worker: &str,
) -> Result<SubstrateProvider, Box<dyn Error>> {
    let mut config = config()?;
    config.router_endpoint = agent_router(agent);
    Ok(provider_over(config, control, index)?.with_worker_id(WorkerId::new(worker)?))
}

fn owned_entry() -> Result<ActorEntry, Box<dyn Error>> {
    let atespace = tenant_atespace()?;
    let lease_id = first_lease();
    let actor = ActorName::for_lease(&lease_id)?;
    Ok(ActorEntry {
        atespace: atespace.as_str().into(),
        actor: actor.as_str().into(),
        lease_id,
        named: false,
        retained: false,
        owner: Some(ActorOwner {
            worker_id: WorkerId::new("substrate-owner-a")?,
            claim_id: ActorClaimId::generate(),
            actor_uid: Some(ActorUid::try_from(ORIGINAL_UID.to_string())?),
        }),
    })
}

fn object_of(entry: &ActorEntry) -> proto::ObjectRef {
    proto::ObjectRef {
        atespace: entry.atespace.clone(),
        name: entry.actor.clone(),
    }
}

fn owner_of(entry: &ActorEntry) -> Result<&WorkerId, Box<dyn Error>> {
    Ok(&entry.owner.as_ref().ok_or("the entry has no owner")?.worker_id)
}

#[tokio::test]
async fn authority_recovery_never_adopts_the_same_live_actor_on_a_second_replica() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let entry = owned_entry()?;
    index.put(&entry).await?;
    let control = Arc::new(FakeControl::running().with_uid(ORIGINAL_UID));
    let first = worker(control.clone(), index.clone(), owner_of(&entry)?.as_str())?;
    let second = worker(control.clone(), index.clone(), "substrate-owner-b")?;

    let recovered_a = recovery::recover(&first).await?;
    let recovered_b = recovery::recover(&second).await?;

    assert_eq!(recovered_a.adopted, 1);
    assert_eq!(recovered_b.adopted, 0);
    assert!(first.active_lease(&entry.lease_id).await.is_some());
    assert!(second.active_lease(&entry.lease_id).await.is_none());
    assert_eq!(index.get(&entry.atespace, &entry.actor).await?, Some(entry));
    assert_eq!(control.suspend_count(), 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn authority_recovery_refuses_a_recreated_actor_with_the_same_name_and_a_new_uid() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let entry = owned_entry()?;
    index.put(&entry).await?;
    let control = Arc::new(FakeControl::running().with_uid(REPLACEMENT_UID));
    let provider = worker(control.clone(), index.clone(), owner_of(&entry)?.as_str())?;

    let report = recovery::recover(&provider).await?;

    assert_eq!(report.adopted, 0);
    assert!(provider.active_lease(&entry.lease_id).await.is_none());
    assert_eq!(index.get(&entry.atespace, &entry.actor).await?, Some(entry));
    assert_eq!(control.suspend_count(), 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

/// What the claim and the control plane disagree about.
enum Incomplete {
    /// The claim has no owner at all.
    Ownerless,
    /// The owner never recorded the actor's UID.
    UnrecordedUid,
    /// The control plane reports no UID.
    ServerWithoutUid,
}

async fn incomplete_custody(mode: Incomplete) -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let mut entry = owned_entry()?;
    let mut control = FakeControl::running();
    match mode {
        Incomplete::Ownerless => {
            entry.owner = None;
            control = control.with_uid(ORIGINAL_UID);
        }
        Incomplete::UnrecordedUid => {
            entry.owner.as_mut().ok_or("the entry has no owner")?.actor_uid = None;
            control = control.with_uid(ORIGINAL_UID);
        }
        Incomplete::ServerWithoutUid => {}
    }
    index.put(&entry).await?;
    let control = Arc::new(control);
    let provider = worker(control.clone(), index.clone(), "substrate-owner-a")?;

    let report = recovery::recover(&provider).await?;

    assert_eq!(report.adopted, 0);
    assert!(provider.active_lease(&entry.lease_id).await.is_none());
    assert_eq!(index.get(&entry.atespace, &entry.actor).await?, Some(entry));
    assert_eq!(control.suspend_count(), 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn authority_recovery_keeps_legacy_unbound_claims_unconfirmed() -> TestResult {
    incomplete_custody(Incomplete::Ownerless).await
}

#[tokio::test]
async fn authority_recovery_keeps_claims_without_a_recorded_uid_unconfirmed() -> TestResult {
    incomplete_custody(Incomplete::UnrecordedUid).await
}

#[tokio::test]
async fn authority_recovery_keeps_missing_server_uid_unconfirmed() -> TestResult {
    incomplete_custody(Incomplete::ServerWithoutUid).await
}

#[tokio::test]
async fn authority_failed_suspend_keeps_the_same_actor_claim_available_for_retry() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let entry = owned_entry()?;
    index.put(&entry).await?;
    let control = Arc::new(FakeControl::running().with_uid(ORIGINAL_UID));
    let provider = worker(control.clone(), index.clone(), owner_of(&entry)?.as_str())?;
    assert_eq!(recovery::recover(&provider).await?.adopted, 1);
    control.fail_get_with(AteError::Unavailable {
        message: "owned API outage".into(),
    });

    let result = trogon_sandbox_protocol::Provider::cancel(&provider, &entry.lease_id).await;

    assert!(result.is_err());
    assert!(provider.active_lease(&entry.lease_id).await.is_some());
    assert_eq!(index.get(&entry.atespace, &entry.actor).await?, Some(entry));
    assert_eq!(control.delete_count(), 0);
    Ok(())
}
