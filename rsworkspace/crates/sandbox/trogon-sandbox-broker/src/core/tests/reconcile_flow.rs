use super::*;
use crate::entity_shadow::{EntityShadow, InMemoryEntityEventSink};
use crate::entity_store::{InMemorySandboxEntityStore, NewSandboxEntity, SandboxEntityMode, SandboxEntityStore};
use trogon_sandbox_contract::EntityState;
use trogon_sandbox_contract::SandboxEntityId;

pub(super) fn core_over(
    mode: SandboxEntityMode,
    response: Bytes,
) -> anyhow::Result<(BrokerCore, Arc<InMemorySandboxEntityStore>)> {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let shadow = EntityShadow::new(
        mode,
        store.clone(),
        Arc::new(InMemoryEntityEventSink::new()),
        Duration::from_millis(10),
    )
    .with_head_poll(Duration::from_millis(10), 2);
    Ok((make_core(response)?.with_entity_shadow(Arc::new(shadow)), store))
}

/// An unnamed entity put straight into `state`, as an entity verb would leave it.
pub(super) async fn entity_in(
    store: &InMemorySandboxEntityStore,
    tenant: &TenantId,
    state: EntityState,
) -> anyhow::Result<SandboxEntityId> {
    let spec = running_source_record(tenant, &LeaseId::new("template"))?.spec;
    let created = store
        .create(NewSandboxEntity {
            tenant: tenant.clone(),
            name: None,
            created_by: None,
            spec,
            metadata: SandboxTags::default(),
            drives: vec![],
            persistence_tag: None,
        })
        .await?;
    let id = created.record.entity_id.clone();
    store
        .mutate(tenant, &id, &|record| {
            record.current_lease = state.lease().cloned();
            record.state = state.clone();
            Ok(())
        })
        .await?;
    Ok(id)
}

pub(super) async fn bound_lease(
    core: &BrokerCore,
    tenant: &TenantId,
    id: &str,
    entity: &SandboxEntityId,
) -> anyhow::Result<LeaseId> {
    let lease_id = LeaseId::new(id);
    let mut record = running_source_record(tenant, &lease_id)?;
    record.entity_id = Some(entity.clone());
    core.active_leases.put(&record).await?;
    Ok(lease_id)
}

pub(super) async fn state_of(
    store: &InMemorySandboxEntityStore,
    tenant: &TenantId,
    id: &SandboxEntityId,
) -> anyhow::Result<EntityState> {
    Ok(store
        .get(tenant, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the entity is missing"))?
        .record
        .state)
}

#[tokio::test]
async fn nothing_is_reconciled_while_the_legacy_stores_decide() -> TestResult {
    let (core, store) = core_over(SandboxEntityMode::Shadow, mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(&store, &tenant, EntityState::Deleting).await?;

    let report = core.reconcile_entities().await;

    assert_eq!(report, Default::default());
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Deleting);
    Ok(())
}

#[tokio::test]
async fn an_entity_that_asks_to_be_deleted_is_deleted() -> TestResult {
    let (core, store) = core_over(SandboxEntityMode::On, mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(&store, &tenant, EntityState::Deleting).await?;

    let report = core.reconcile_entities().await;

    assert_eq!(report.deletes_finished, 1);
    assert_eq!(report.failures, 0);
    assert!(store.get(&tenant, &id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_stop_releases_the_lease_and_stops_the_entity() -> TestResult {
    let (core, store) = core_over(SandboxEntityMode::On, mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(
        &store,
        &tenant,
        EntityState::Stopping {
            lease: LeaseId::new("stop-me"),
        },
    )
    .await?;
    let lease = bound_lease(&core, &tenant, "stop-me", &id).await?;

    let report = core.reconcile_entities().await;

    assert_eq!(report.stops_driven, 1);
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Stopped);
    let record = core.active_leases.get(&lease).await?;
    assert!(record.is_none_or(|r| r.status == LeaseStatus::Released));
    Ok(())
}

#[tokio::test]
async fn a_stop_whose_lease_is_already_gone_ends_the_run() -> TestResult {
    let (core, store) = core_over(SandboxEntityMode::On, mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(
        &store,
        &tenant,
        EntityState::Stopping {
            lease: LeaseId::new("long-gone"),
        },
    )
    .await?;

    let report = core.reconcile_entities().await;

    assert_eq!(report.runs_ended, 1);
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Stopped);
    Ok(())
}

#[tokio::test]
async fn a_pause_is_driven_once_and_then_left_alone() -> TestResult {
    let (core, store) = core_over(SandboxEntityMode::On, freeze_reply(SandboxFreezeResponse::Done)?)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(
        &store,
        &tenant,
        EntityState::Paused {
            lease: LeaseId::new("pause-me"),
        },
    )
    .await?;
    let lease = bound_lease(&core, &tenant, "pause-me", &id).await?;

    let first = core.reconcile_entities().await;
    let second = core.reconcile_entities().await;

    assert_eq!(first.pauses_driven, 1);
    assert_eq!(second, Default::default());
    let record = core.active_leases.get(&lease).await?.ok_or("the lease is missing")?;
    assert_eq!(record.status, LeaseStatus::Paused);
    Ok(())
}

#[tokio::test]
async fn a_lease_owned_by_another_tenant_is_not_touched() -> TestResult {
    let (core, store) = core_over(SandboxEntityMode::On, mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(
        &store,
        &tenant,
        EntityState::Stopping {
            lease: LeaseId::new("foreign"),
        },
    )
    .await?;
    let foreign = bound_lease(&core, &TenantId::new("other")?, "foreign", &id).await?;

    let report = core.reconcile_entities().await;

    assert_eq!(report.stops_driven, 0);
    assert_eq!(report.runs_ended, 1);
    let record = core.active_leases.get(&foreign).await?.ok_or("the lease is missing")?;
    assert_eq!(record.status, LeaseStatus::Running);
    Ok(())
}
