//! Entity checks: a persistent sandbox entity seen through a real broker core that leases from a real worker over
//! NATS.
//!
//! Entity mode lives in the broker, not the worker, so the provider-subject suite cannot reach it. These checks run a
//! broker core in process (in-memory stores, the entity store in the mode under test) wired to the worker through the
//! NATS bus, then drive named persistent leases and read the entity record back. They prove the worker honours what
//! the entity lifecycle relies on: a grant per run, a cancel that ends the run, and the same entity across runs.

use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_broker::active_lease::{ActiveLeaseStore, InMemoryActiveLeaseStore};
use trogon_sandbox_broker::bus::BrokerBus;
use trogon_sandbox_broker::core::{BrokerConfig, BrokerCore, InMemoryDriveStore};
use trogon_sandbox_broker::digest_pinning::InMemorySnapshotPinStore;
use trogon_sandbox_broker::entity_shadow::{EntityShadow, InMemoryEntityEventSink};
use trogon_sandbox_broker::entity_store::{InMemorySandboxEntityStore, SandboxEntityMode, SandboxEntityStore};
use trogon_sandbox_broker::host_import_policy::InMemoryHostImportPolicy;
use trogon_sandbox_broker::host_mount_policy::InMemoryHostMountAllowList;
use trogon_sandbox_broker::idempotency::InMemoryIdempotencyStore;
use trogon_sandbox_broker::image_policy::InMemoryImageAllowList;
use trogon_sandbox_broker::limits::InMemoryTenantLimits;
use trogon_sandbox_broker::metrics::BrokerMetrics;
use trogon_sandbox_broker::named_lease::InMemoryNamedLeaseStore;
use trogon_sandbox_broker::nats_bus::NatsBus;
use trogon_sandbox_broker::quota::InMemoryQuotaStore;
use trogon_sandbox_broker::resolved_digest::InMemoryResolvedDigestStore;
use trogon_sandbox_broker::signing_bus::SigningBus;
use trogon_sandbox_broker::snapshot_tag::{InMemorySnapshotTagAuditor, InMemorySnapshotTagStore};
use trogon_sandbox_broker::worker_registry::InMemoryWorkerRegistry;
use trogon_sandbox_contract::{
    DeleteEntityRequest, DeleteEntityResponse, DeleteLeaseRequest, DeleteLeaseResponse, EntityLifecycleEvent,
    EntityRef, EntityState, LeaseError, LeaseGranted, LeaseId, LeaseName, LeasePersistence, LeasePrewarm, LeaseRequest,
    LeaseRequestKey, LeaseResponse, OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts,
    SandboxTags, TenantId, WorkspaceSource,
};
use trogon_sandbox_memory_worker::MemoryWorker;
use trogon_sandbox_protocol::DispatchSigner;

use super::Fallible;

const NAME: &str = "conformance-workspace";

fn tenant() -> Fallible<TenantId> {
    Ok(TenantId::new("entity-conformance")?)
}

fn named_request(key: &str) -> Fallible<LeaseRequest> {
    let mut request = super::request::lease_request(tenant()?, key)?;
    request.name = Some(LeaseName::new(NAME)?);
    request.persistent = LeasePersistence::Persistent;
    Ok(request)
}

/// A broker core with the entity store and the lifecycle events it writes.
pub struct Broker {
    pub core: BrokerCore,
    pub store: Arc<InMemorySandboxEntityStore>,
    pub events: Arc<InMemoryEntityEventSink>,
}

/// A broker core over the NATS bus. With a `signer` it signs lease dispatches and per-lease requests as the broker
/// binary does when a key is configured.
pub fn broker(
    client: async_nats::Client,
    mode: SandboxEntityMode,
    signer: Option<Arc<DispatchSigner>>,
) -> Fallible<Broker> {
    let leases: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let nats = NatsBus::new(client);
    let bus: Arc<dyn BrokerBus> = match &signer {
        Some(signer) => Arc::new(SigningBus::new(Arc::new(nats), Arc::clone(&leases), Arc::clone(signer))),
        None => Arc::new(nats),
    };
    let mut core = BrokerCore::new(
        bus,
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(InMemoryIdempotencyStore::new()),
        leases,
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        InMemorySnapshotPinStore::new(),
        Arc::new(InMemorySnapshotTagStore::new(Arc::new(
            InMemorySnapshotTagAuditor::new(),
        ))),
        Arc::new(InMemoryResolvedDigestStore::new()),
        Arc::new(InMemoryDriveStore::new()),
        Arc::new(InMemoryNamedLeaseStore::new()),
        BrokerConfig {
            default_cpu_cap: 3600,
            default_wall_cap: 7200,
            enforce_image_allowlist: false,
            enforce_worker_registry: false,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 50,
            snapshot_registry: "snapshots.local".to_owned(),
            provider_dispatch_timeout: Duration::from_secs(10),
        },
        BrokerMetrics::new()?,
    );
    if let Some(signer) = signer {
        core = core.with_dispatch_signer(signer);
    }
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let events = Arc::new(InMemoryEntityEventSink::new());
    let shadow = EntityShadow::new(mode, store.clone(), events.clone(), Duration::from_millis(10))
        .with_head_poll(Duration::from_millis(10), 5);
    Ok(Broker {
        core: core.with_entity_shadow(Arc::new(shadow)),
        store,
        events,
    })
}

async fn lease(broker: &Broker, key: &str) -> Fallible<LeaseGranted> {
    match broker.core.handle_lease_request(named_request(key)?).await {
        LeaseResponse::Granted(grant) => Ok(grant),
        other => Err(format!("named lease was not granted: {other:?}").into()),
    }
}

async fn entity_state(broker: &Broker) -> Fallible<(EntityState, u64)> {
    let entity = broker
        .store
        .get_by_name(&tenant()?, NAME)
        .await?
        .ok_or("the lease left no entity behind")?;
    Ok((entity.record.state, entity.record.generation))
}

async fn wait_for_leases(worker: &MemoryWorker, expected: usize) -> Fallible<()> {
    for _ in 0..100 {
        if worker.lease_count() == expected {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Err(format!("worker holds {} leases, expected {expected}", worker.lease_count()).into())
}

async fn end_run(broker: &Broker, lease: &LeaseId) -> Fallible<()> {
    match broker
        .core
        .handle_delete_lease(DeleteLeaseRequest {
            tenant: tenant()?,
            lease_id: lease.clone(),
            operation_id: None,
        })
        .await
    {
        DeleteLeaseResponse::Deleted { .. } => Ok(()),
        DeleteLeaseResponse::Denied(error) => Err(format!("lease delete was denied: {error:?}").into()),
    }
}

async fn binds_an_entity(broker: &Broker, worker: &MemoryWorker, first: &LeaseGranted) -> Fallible<()> {
    let id = first.entity_id.clone().ok_or("the grant names no entity")?;
    let (state, generation) = entity_state(broker).await?;
    let running = EntityState::Running {
        lease: first.lease_id.clone(),
    };
    if state != running || generation != 1 {
        return Err(format!("entity is {state:?} at generation {generation}").into());
    }
    let stored = broker
        .store
        .get_by_name(&tenant()?, NAME)
        .await?
        .ok_or("entity vanished")?;
    if stored.record.entity_id != id {
        return Err("the grant and the store name different entities".into());
    }
    wait_for_leases(worker, 1).await
}

async fn ending_stops(broker: &Broker, worker: &MemoryWorker, first: &LeaseGranted) -> Fallible<()> {
    end_run(broker, &first.lease_id).await?;
    wait_for_leases(worker, 0).await?;
    let (state, _) = entity_state(broker).await?;
    if state != EntityState::Stopped {
        return Err(format!("entity is {state:?} after the run ended").into());
    }
    Ok(())
}

async fn survives_a_restart(
    broker: &Broker,
    worker: &MemoryWorker,
    first: &LeaseGranted,
    second: &LeaseGranted,
) -> Fallible<()> {
    if second.entity_id != first.entity_id || second.lease_id == first.lease_id {
        return Err("a restart must keep the entity and take a new lease".into());
    }
    let (state, generation) = entity_state(broker).await?;
    let running = EntityState::Running {
        lease: second.lease_id.clone(),
    };
    if state != running || generation != 2 {
        return Err(format!("entity is {state:?} at generation {generation}").into());
    }
    let started = broker
        .events
        .events()
        .iter()
        .filter(|event| matches!(event, EntityLifecycleEvent::RunStarted { .. }))
        .count();
    if started != 2 {
        return Err(format!("{started} run-started events, expected 2").into());
    }
    wait_for_leases(worker, 1).await
}

async fn delete_follows_the_mode(
    broker: &Broker,
    worker: &MemoryWorker,
    mode: SandboxEntityMode,
    second: &LeaseGranted,
) -> Fallible<()> {
    end_run(broker, &second.lease_id).await?;
    wait_for_leases(worker, 0).await?;
    let id = second.entity_id.clone().ok_or("the grant names no entity")?;
    let deleted = broker
        .core
        .handle_delete_entity(DeleteEntityRequest {
            tenant: tenant()?,
            entity: EntityRef::Id(id),
        })
        .await;
    match (mode, deleted) {
        (SandboxEntityMode::On, DeleteEntityResponse::Deleted) => {
            if broker.store.get_by_name(&tenant()?, NAME).await?.is_none() {
                Ok(())
            } else {
                Err("a deleted entity is still in the store".into())
            }
        }
        (SandboxEntityMode::Shadow, DeleteEntityResponse::Denied(LeaseError::EntityNotEnabled)) => Ok(()),
        (mode, other) => Err(format!("{mode:?} mode answered a delete with {other:?}").into()),
    }
}

fn record(failures: &mut Vec<String>, name: &str, result: Fallible<()>) {
    if let Err(why) = result {
        failures.push(format!("{name}: {why}"));
    }
}

/// Runs every entity check against `worker` in the broker's entity `mode` and returns the failures as
/// `name: reason`.
pub async fn run_entity_checks(
    worker: &MemoryWorker,
    client: async_nats::Client,
    mode: SandboxEntityMode,
    signer: Option<Arc<DispatchSigner>>,
) -> Fallible<Vec<String>> {
    let broker = broker(client, mode, signer)?;
    let mut failures = Vec::new();
    let first = match lease(&broker, "first").await {
        Ok(grant) => grant,
        Err(why) => return Ok(vec![format!("a_named_lease_reaches_the_worker: {why}")]),
    };
    record(
        &mut failures,
        "a_named_lease_binds_an_entity_to_a_live_worker_lease",
        binds_an_entity(&broker, worker, &first).await,
    );
    record(
        &mut failures,
        "ending_the_run_releases_the_worker_lease_and_stops_the_entity",
        ending_stops(&broker, worker, &first).await,
    );
    let second = match lease(&broker, "second").await {
        Ok(grant) => grant,
        Err(why) => {
            record(
                &mut failures,
                "the_entity_survives_a_restart_with_a_new_lease",
                Err(why),
            );
            return Ok(failures);
        }
    };
    record(
        &mut failures,
        "the_entity_survives_a_restart_with_a_new_lease",
        survives_a_restart(&broker, worker, &first, &second).await,
    );
    record(
        &mut failures,
        "entity_delete_follows_the_mode",
        delete_follows_the_mode(&broker, worker, mode, &second).await,
    );
    Ok(failures)
}
