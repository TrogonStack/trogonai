use super::*;
use trogon_sandbox_contract::{OciArtifactRef, RollbackLeaseRequest, RollbackLeaseResponse, SandboxRollbackResponse};

/// Answers rollback requests with a scripted reply and records the subjects.
struct RollbackBus {
    reply: SandboxRollbackResponse,
    subjects: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for RollbackBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        locked(&self.subjects)?.push(subject.to_string());
        if subject.starts_with("sandbox.provider.mock.rollback.") {
            return Ok(serde_json::to_vec(&self.reply)?.into());
        }
        Err(anyhow::anyhow!("unexpected subject {subject}"))
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn core_with(
    offers_rollback: bool,
    reply: SandboxRollbackResponse,
) -> anyhow::Result<(BrokerCore, Arc<RollbackBus>)> {
    let bus = Arc::new(RollbackBus {
        reply,
        subjects: Default::default(),
    });
    let mut core = make_core_with_bus(bus.clone())?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.snapshot.supports_in_place_rollback = CapabilitySupport::from(offers_rollback);
    let mut heartbeat = worker_heartbeat("rollback", ProviderKind::Mock, profile)?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    Ok((core, bus))
}

fn request(
    core: &BrokerCore,
    tenant: &TenantId,
    lease_id: &LeaseId,
    repository: String,
) -> anyhow::Result<RollbackLeaseRequest> {
    Ok(RollbackLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        snapshot: OciArtifactRef {
            registry: core.config.snapshot_registry.clone(),
            repository,
            digest: format!("sha256:{}", "a".repeat(64)).parse()?,
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        operation_id: None,
    })
}

#[tokio::test]
async fn a_rollback_asks_the_worker_and_leaves_the_record_alone() -> TestResult {
    let (core, bus) = core_with(true, SandboxRollbackResponse::Done).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "rollback-ok").await?;
    let before = core.active_leases.get(&lease_id).await?.context("lease record")?;
    let own = snapshot_repository_for_tenant(&tenant);

    let response = core
        .handle_rollback_lease(request(&core, &tenant, &lease_id, own)?)
        .await;
    assert!(matches!(
        response,
        RollbackLeaseResponse::RolledBack {
            operation_id: None | Some(_)
        }
    ));
    assert_eq!(locked(&bus.subjects)?.len(), 1);
    let after = core.active_leases.get(&lease_id).await?.context("lease record")?;
    assert_eq!(after.status, LeaseStatus::Running);
    assert_eq!(after.deadline, before.deadline);
    Ok(())
}

#[tokio::test]
async fn another_tenants_snapshot_is_refused_before_any_worker_is_asked() -> TestResult {
    let (core, bus) = core_with(true, SandboxRollbackResponse::Done).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "rollback-foreign").await?;
    let foreign = snapshot_repository_for_tenant(&TenantId::new("other")?);

    let response = core
        .handle_rollback_lease(request(&core, &tenant, &lease_id, foreign)?)
        .await;
    assert!(matches!(
        response,
        RollbackLeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    assert!(locked(&bus.subjects)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_provider_without_rollback_answers_unsupported() -> TestResult {
    let (core, bus) = core_with(false, SandboxRollbackResponse::Done).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "rollback-unsupported").await?;
    let own = snapshot_repository_for_tenant(&tenant);

    let response = core
        .handle_rollback_lease(request(&core, &tenant, &lease_id, own)?)
        .await;
    assert_eq!(response, RollbackLeaseResponse::Unsupported);
    assert!(locked(&bus.subjects)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_worker_denial_reaches_the_caller_and_the_lease_keeps_running() -> TestResult {
    let denial = LeaseError::Backend {
        message: "snapshot is gone".into(),
    };
    let (core, _bus) = core_with(true, SandboxRollbackResponse::Denied(denial.clone())).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "rollback-denied").await?;
    let own = snapshot_repository_for_tenant(&tenant);

    let response = core
        .handle_rollback_lease(request(&core, &tenant, &lease_id, own)?)
        .await;
    assert_eq!(response, RollbackLeaseResponse::Denied(denial));
    let after = core.active_leases.get(&lease_id).await?.context("lease record")?;
    assert_eq!(after.status, LeaseStatus::Running);
    Ok(())
}
