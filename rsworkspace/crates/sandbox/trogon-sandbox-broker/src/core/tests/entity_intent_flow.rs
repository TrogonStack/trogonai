//! The stop, pause, resume and delete verbs record desired state on the
//! entity before they act, and the reconciler finishes what a restart cut off.

use super::reconcile_flow::{bound_lease, core_over, entity_in, state_of};
use super::*;
use crate::entity_shadow::Intent;
use crate::entity_store::{SandboxEntityMode, SandboxEntityStore};
use trogon_sandbox_contract::SandboxEntityId;
use trogon_sandbox_contract::{DeleteEntityRequest, DeleteEntityResponse, EntityRef, EntityState};
use trogon_sandbox_contract::{ResumeLeaseRequest, StopLeaseRequest};
use trogon_sandbox_protocol::EntityEvent;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn done() -> anyhow::Result<Bytes> {
    freeze_reply(SandboxFreezeResponse::Done)
}

fn pause_request(tenant: &TenantId, lease: &LeaseId) -> PauseLeaseRequest {
    PauseLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease.clone(),
        max_paused_seconds: None,
        operation_id: None,
    }
}

fn resume_request(tenant: &TenantId, lease: &LeaseId) -> ResumeLeaseRequest {
    ResumeLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease.clone(),
        operation_id: None,
    }
}

async fn running_entity(
    mode: SandboxEntityMode,
    response: Bytes,
    lease: &str,
) -> anyhow::Result<(
    BrokerCore,
    Arc<crate::entity_store::InMemorySandboxEntityStore>,
    TenantId,
    SandboxEntityId,
    LeaseId,
)> {
    let (core, store) = core_over(mode, response)?;
    let tenant = TenantId::new("acme")?;
    let id = entity_in(
        &store,
        &tenant,
        EntityState::Running {
            lease: LeaseId::new(lease),
        },
    )
    .await?;
    let lease = bound_lease(&core, &tenant, lease, &id).await?;
    Ok((core, store, tenant, id, lease))
}

#[tokio::test]
async fn a_stop_ends_the_entity_run_in_both_modes() -> TestResult {
    for mode in [SandboxEntityMode::Shadow, SandboxEntityMode::On] {
        let (core, store, tenant, id, lease) = running_entity(mode, mock_provider_response()?, "stop-me").await?;

        let response = core
            .handle_stop_lease(StopLeaseRequest {
                tenant: tenant.clone(),
                lease_id: lease.clone(),
            })
            .await;

        assert_eq!(response, StopLeaseResponse::Stopped, "{mode:?}");
        assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Stopped, "{mode:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_stop_the_broker_died_inside_is_finished_by_the_reconciler() -> TestResult {
    let (core, store, tenant, id, lease) =
        running_entity(SandboxEntityMode::On, mock_provider_response()?, "cut-off").await?;
    // The verb wrote its intent and the broker died before releasing the lease.
    core.entity_intend(&tenant, &lease, Intent::Stop).await?;
    assert_eq!(
        state_of(&store, &tenant, &id).await?,
        EntityState::Stopping { lease: lease.clone() }
    );

    let report = core.reconcile_entities().await;

    assert_eq!(report.stops_driven, 1);
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Stopped);
    let record = core.active_leases.get(&lease).await?;
    assert!(record.is_none_or(|r| r.status == LeaseStatus::Released));
    assert_eq!(core.reconcile_entities().await, Default::default());
    Ok(())
}

#[tokio::test]
async fn a_pause_is_recorded_in_both_modes_and_only_on_mode_reconciles() -> TestResult {
    for mode in [SandboxEntityMode::Shadow, SandboxEntityMode::On] {
        let (core, store, tenant, id, lease) = running_entity(mode, done()?, "pause-me").await?;

        let response = core.handle_pause_lease(pause_request(&tenant, &lease)).await;

        assert!(matches!(response, PauseLeaseResponse::Paused { .. }), "{mode:?}");
        assert_eq!(
            state_of(&store, &tenant, &id).await?,
            EntityState::Paused { lease: lease.clone() },
            "{mode:?}"
        );
        let record = core
            .active_leases
            .get(&lease)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?;
        assert_eq!(record.status, LeaseStatus::Paused);
        assert_eq!(core.reconcile_entities().await, Default::default());
    }
    Ok(())
}

#[tokio::test]
async fn a_pause_the_provider_refuses_leaves_the_entity_running() -> TestResult {
    let (core, store, tenant, id, lease) = running_entity(
        SandboxEntityMode::On,
        freeze_reply(SandboxFreezeResponse::Unsupported)?,
        "no-pause",
    )
    .await?;

    let response = core.handle_pause_lease(pause_request(&tenant, &lease)).await;

    assert_eq!(response, PauseLeaseResponse::Unsupported);
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Running { lease });
    assert_eq!(core.reconcile_entities().await, Default::default());
    Ok(())
}

#[tokio::test]
async fn a_pause_the_broker_died_inside_is_finished_by_a_fresh_reconcile() -> TestResult {
    let (core, store, tenant, id, lease) = running_entity(SandboxEntityMode::On, done()?, "cut-pause").await?;
    core.entity_intend(&tenant, &lease, Intent::Pause).await?;
    assert_eq!(
        core.active_leases
            .get(&lease)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?
            .status,
        LeaseStatus::Running
    );

    let report = core.reconcile_entities().await;

    assert_eq!(report.pauses_driven, 1);
    assert_eq!(
        core.active_leases
            .get(&lease)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?
            .status,
        LeaseStatus::Paused
    );
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Paused { lease });
    assert_eq!(core.reconcile_entities().await, Default::default());
    Ok(())
}

#[tokio::test]
async fn a_resume_writes_the_running_state_back() -> TestResult {
    for mode in [SandboxEntityMode::Shadow, SandboxEntityMode::On] {
        let (core, store, tenant, id, lease) = running_entity(mode, done()?, "resume-me").await?;
        core.handle_pause_lease(pause_request(&tenant, &lease)).await;

        let response = core.handle_resume_lease(resume_request(&tenant, &lease)).await;

        assert!(matches!(response, ResumeLeaseResponse::Resumed { .. }), "{mode:?}");
        assert_eq!(
            state_of(&store, &tenant, &id).await?,
            EntityState::Running { lease: lease.clone() },
            "{mode:?}"
        );
        let record = core
            .active_leases
            .get(&lease)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?;
        assert_eq!(record.status, LeaseStatus::Running);
        assert_eq!(core.reconcile_entities().await, Default::default());
    }
    Ok(())
}

#[tokio::test]
async fn a_resume_the_broker_died_inside_is_finished_by_a_fresh_reconcile() -> TestResult {
    let (core, store, tenant, id, lease) = running_entity(SandboxEntityMode::On, done()?, "cut-resume").await?;
    core.handle_pause_lease(pause_request(&tenant, &lease)).await;
    core.entity_intend(&tenant, &lease, Intent::Resume).await?;
    assert_eq!(
        state_of(&store, &tenant, &id).await?,
        EntityState::Running { lease: lease.clone() }
    );

    let report = core.reconcile_entities().await;

    assert_eq!(report.resumes_driven, 1);
    assert_eq!(
        core.active_leases
            .get(&lease)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?
            .status,
        LeaseStatus::Running
    );
    assert_eq!(core.reconcile_entities().await, Default::default());
    Ok(())
}

#[tokio::test]
async fn a_pause_by_the_idle_evaluator_is_not_undone() -> TestResult {
    let (core, store, tenant, id, lease) = running_entity(SandboxEntityMode::On, done()?, "idle").await?;
    let mut record = core
        .active_leases
        .get(&lease)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?;
    record.status = LeaseStatus::Paused;
    record.auto_paused_at = Some(SystemTime::now());
    core.active_leases.put(&record).await?;

    assert_eq!(core.reconcile_entities().await, Default::default());
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Running { lease });
    Ok(())
}

#[tokio::test]
async fn a_delete_records_the_intent_and_removes_the_entity_and_its_lease() -> TestResult {
    let (core, store, tenant, id, lease) =
        running_entity(SandboxEntityMode::On, mock_provider_response()?, "delete-me").await?;

    let response = core
        .handle_delete_entity(DeleteEntityRequest {
            tenant: tenant.clone(),
            entity: EntityRef::Id(id.clone()),
        })
        .await;

    assert_eq!(response, DeleteEntityResponse::Deleted);
    assert!(store.get(&tenant, &id).await?.is_none());
    let record = core.active_leases.get(&lease).await?;
    assert!(record.is_none_or(|r| r.status == LeaseStatus::Released));
    Ok(())
}

#[tokio::test]
async fn a_delete_the_broker_died_inside_still_ends_the_lease_and_the_entity() -> TestResult {
    let (core, store, tenant, id, lease) =
        running_entity(SandboxEntityMode::On, mock_provider_response()?, "cut-delete").await?;
    // The verb wrote `Deleting` and the broker died before purging anything.
    core.entity_intend_delete(&tenant, &EntityRef::Id(id.clone())).await?;
    let stranded = store
        .get(&tenant, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the lease is missing"))?
        .record;
    assert_eq!(stranded.state, EntityState::Deleting);
    assert_eq!(stranded.current_lease, Some(lease.clone()));

    let report = core.reconcile_entities().await;

    assert_eq!(report.deletes_finished, 1);
    assert!(store.get(&tenant, &id).await?.is_none());
    let record = core.active_leases.get(&lease).await?;
    assert!(record.is_none_or(|r| r.status == LeaseStatus::Released));
    Ok(())
}

#[tokio::test]
async fn a_delete_intent_in_shadow_mode_is_recorded_but_not_acted_on() -> TestResult {
    let (core, store, tenant, id, _lease) =
        running_entity(SandboxEntityMode::Shadow, mock_provider_response()?, "shadow-del").await?;
    core.entity_intend_delete(&tenant, &EntityRef::Id(id.clone())).await?;

    assert_eq!(core.reconcile_entities().await, Default::default());
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Deleting);
    Ok(())
}

#[tokio::test]
async fn an_intent_for_a_lease_the_entity_has_moved_off_is_ignored() -> TestResult {
    let (core, store, tenant, id, lease) = running_entity(SandboxEntityMode::On, done()?, "old-run").await?;
    store
        .apply_event(&tenant, &id, &EntityEvent::RunEnded { lease: lease.clone() })
        .await?;

    let written = core.entity_intend(&tenant, &lease, Intent::Pause).await?;

    assert!(written.is_none());
    assert_eq!(state_of(&store, &tenant, &id).await?, EntityState::Stopped);
    Ok(())
}
