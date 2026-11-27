use super::*;
use std::collections::VecDeque;
use trogon_sandbox_contract::{
    DormancyCapability, SandboxSuspendResponse, SandboxWakeResponse, SnapshotCaptureAccess, SuspendLeaseRequest,
    SuspendLeaseResponse, SuspendLevel, SuspendRequestLevel, WakeLeaseRequest, WakeLeaseResponse,
};

/// Answers suspend and wake requests from scripted queues and records
/// every subject it was asked on. An empty queue answers `Done`.
struct DormancyBus {
    suspend: std::sync::Mutex<VecDeque<Result<SandboxSuspendResponse, ()>>>,
    wake: std::sync::Mutex<VecDeque<Result<SandboxWakeResponse, ()>>>,
    subjects: std::sync::Mutex<Vec<String>>,
}

impl DormancyBus {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            suspend: Default::default(),
            wake: Default::default(),
            subjects: Default::default(),
        })
    }

    fn script_suspend(&self, reply: Result<SandboxSuspendResponse, ()>) -> anyhow::Result<()> {
        locked(&self.suspend)?.push_back(reply);
        Ok(())
    }

    fn script_wake(&self, reply: Result<SandboxWakeResponse, ()>) -> anyhow::Result<()> {
        locked(&self.wake)?.push_back(reply);
        Ok(())
    }

    fn asked(&self, prefix: &str) -> anyhow::Result<usize> {
        Ok(locked(&self.subjects)?.iter().filter(|s| s.starts_with(prefix)).count())
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for DormancyBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        locked(&self.subjects)?.push(subject.to_string());
        if subject.starts_with("sandbox.provider.mock.suspend.") {
            let next = locked(&self.suspend)?.pop_front();
            let reply = next.unwrap_or(Ok(SandboxSuspendResponse::Done {
                artifact: None,
                artifact_bytes: Some(4096),
            }));
            let reply = reply.map_err(|()| anyhow::anyhow!("injected timeout"))?;
            return Ok(serde_json::to_vec(&reply)?.into());
        }
        if subject.starts_with("sandbox.provider.mock.wake.") {
            let next = locked(&self.wake)?.pop_front();
            let reply = next.unwrap_or(Ok(SandboxWakeResponse::Done));
            let reply = reply.map_err(|()| anyhow::anyhow!("injected timeout"))?;
            return Ok(serde_json::to_vec(&reply)?.into());
        }
        Err(anyhow::anyhow!("unexpected subject {subject}"))
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

async fn core_offering(dormancy: DormancyCapability) -> anyhow::Result<(BrokerCore, Arc<DormancyBus>)> {
    let bus = DormancyBus::new();
    let mut core = make_core_with_bus(bus.clone())?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.lifecycle.dormancy = dormancy;
    let mut heartbeat = worker_heartbeat("dormant", ProviderKind::Mock, profile)?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    Ok((core, bus))
}

fn suspend_req(tenant: &TenantId, lease_id: &LeaseId, level: SuspendRequestLevel) -> SuspendLeaseRequest {
    SuspendLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        level,
        max_suspended_seconds: None,
        operation_id: None,
    }
}

fn wake_req(tenant: &TenantId, lease_id: &LeaseId) -> WakeLeaseRequest {
    WakeLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        operation_id: None,
    }
}

async fn stored(core: &BrokerCore, lease_id: &LeaseId) -> Result<LeaseRecord, Box<dyn Error>> {
    Ok(core.active_leases.get(lease_id).await?.ok_or("lease record missing")?)
}

#[tokio::test]
async fn suspend_then_wake_round_trips_at_each_level() -> TestResult {
    for (capability, level) in [
        (DormancyCapability::SUSPEND_MEMORY, SuspendLevel::Memory),
        (DormancyCapability::SUSPEND_DISK, SuspendLevel::Disk),
    ] {
        let (core, bus) = core_offering(capability).await?;
        let tenant = TenantId::new("acme")?;
        let lease_id = running_lease_in(&core, &tenant, "dormant-roundtrip").await?;

        let SuspendLeaseResponse::Suspended {
            level: got,
            reap_at,
            operation_id: None,
        } = core
            .handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
            .await
        else {
            return Err(format!("suspend should succeed at {level:?}").into());
        };
        assert_eq!(got, level);
        let suspended = stored(&core, &lease_id).await?;
        assert_eq!(suspended.status, LeaseStatus::Suspended);
        assert_eq!(suspended.deadline, reap_at);
        assert!(reap_at > SystemTime::now() + Duration::from_secs(86_000));
        let dormancy = suspended.dormancy.clone().ok_or("dormancy recorded")?;
        assert_eq!(dormancy.level, level);
        assert_eq!(dormancy.generation, 1);
        assert_eq!(dormancy.artifact_bytes, Some(4096));
        assert!((298..=300).contains(&dormancy.run_remaining_seconds));

        // A repeat changes nothing and asks no worker.
        let again = core
            .handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
            .await;
        assert_eq!(
            again,
            SuspendLeaseResponse::Suspended {
                level,
                reap_at,
                operation_id: None
            }
        );
        assert_eq!(bus.asked("sandbox.provider.mock.suspend.")?, 1);

        let WakeLeaseResponse::Woken {
            deadline,
            operation_id: None,
        } = core.handle_wake_lease(wake_req(&tenant, &lease_id)).await
        else {
            return Err("wake should succeed".into());
        };
        let woken = stored(&core, &lease_id).await?;
        assert_eq!(woken.status, LeaseStatus::Running);
        assert_eq!(woken.deadline, deadline);
        let left = deadline.duration_since(SystemTime::now())?.as_secs();
        assert!((297..=300).contains(&left), "left {left}");
        assert!(woken.dormancy.as_ref().ok_or("dormancy recorded")?.woken_at.is_some());

        // Waking a running lease returns it unchanged.
        let again = core.handle_wake_lease(wake_req(&tenant, &lease_id)).await;
        assert_eq!(
            again,
            WakeLeaseResponse::Woken {
                deadline,
                operation_id: None
            }
        );
        assert_eq!(bus.asked("sandbox.provider.mock.wake.")?, 1);

        // A second suspend gets the next generation.
        core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
            .await;
        assert_eq!(
            stored(&core, &lease_id)
                .await?
                .dormancy
                .ok_or("dormancy recorded")?
                .generation,
            2
        );
    }
    Ok(())
}

#[tokio::test]
async fn suspend_refuses_levels_the_provider_does_not_offer() -> TestResult {
    let cases = [
        (DormancyCapability::FREEZE, SuspendRequestLevel::Auto),
        (DormancyCapability::NONE, SuspendRequestLevel::Auto),
        (DormancyCapability::SUSPEND_MEMORY, SuspendRequestLevel::Disk),
        (DormancyCapability::SUSPEND_DISK, SuspendRequestLevel::Memory),
    ];
    for (capability, requested) in cases {
        let (core, bus) = core_offering(capability).await?;
        let tenant = TenantId::new("acme")?;
        let lease_id = running_lease_in(&core, &tenant, "dormant-refused").await?;
        let response = core
            .handle_suspend_lease(suspend_req(&tenant, &lease_id, requested))
            .await;
        assert!(
            matches!(
                response,
                SuspendLeaseResponse::Denied(LeaseError::SuspendUnsupported { .. })
            ),
            "{capability:?} {requested:?}: {response:?}"
        );
        assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Running);
        assert_eq!(bus.asked("sandbox.provider.mock.suspend.")?, 0);
    }
    Ok(())
}

#[tokio::test]
async fn suspend_refuses_when_the_lease_forbids_captures() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("dormant-no-snapshots");
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.spec = with_spec_parts(record.spec, |parts| {
        parts.snapshot_out = None;
        parts.retention.snapshots = SnapshotCaptureAccess::from(false);
    })?;
    core.active_leases.put(&record).await?;
    let response = core
        .handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    assert!(matches!(
        response,
        SuspendLeaseResponse::Denied(LeaseError::SnapshotsDisabled { .. })
    ));
    assert_eq!(bus.asked("sandbox.provider.mock.suspend.")?, 0);
    Ok(())
}

#[tokio::test]
async fn a_refused_suspend_leaves_the_lease_as_it_was() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-worker-refuses").await?;
    let before = stored(&core, &lease_id).await?;

    bus.script_suspend(Ok(SandboxSuspendResponse::Unsupported))?;
    let response = core
        .handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    assert!(matches!(
        response,
        SuspendLeaseResponse::Denied(LeaseError::SuspendUnsupported { .. })
    ));
    let after = stored(&core, &lease_id).await?;
    assert_eq!(after.status, LeaseStatus::Running);
    assert_eq!(after.deadline, before.deadline);

    bus.script_suspend(Ok(SandboxSuspendResponse::Denied(LeaseError::Backend {
        message: "disk full".into(),
    })))?;
    let response = core
        .handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    assert!(matches!(
        response,
        SuspendLeaseResponse::Denied(LeaseError::Backend { .. })
    ));
    assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Running);
    // The next attempt does not reuse the generation of the refused ones.
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    assert_eq!(
        stored(&core, &lease_id)
            .await?
            .dormancy
            .ok_or("dormancy recorded")?
            .generation,
        3
    );
    Ok(())
}

#[tokio::test]
async fn a_paused_lease_suspends_from_its_parked_time_and_a_refusal_keeps_it_paused() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability {
        freeze: CapabilitySupport::Supported,
        suspend: Some(SuspendLevel::Memory),
    })
    .await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("dormant-from-paused");
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.status = LeaseStatus::Paused;
    record.paused_remaining_seconds = Some(120);
    record.deadline = SystemTime::now() + Duration::from_secs(7200);
    core.active_leases.put(&record).await?;

    bus.script_suspend(Ok(SandboxSuspendResponse::Unsupported))?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    let kept = stored(&core, &lease_id).await?;
    assert_eq!(kept.status, LeaseStatus::Paused);
    assert_eq!(kept.paused_remaining_seconds, Some(120));

    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    let suspended = stored(&core, &lease_id).await?;
    assert_eq!(suspended.status, LeaseStatus::Suspended);
    assert_eq!(suspended.paused_remaining_seconds, None);
    assert_eq!(
        suspended.dormancy.ok_or("dormancy recorded")?.run_remaining_seconds,
        120
    );

    core.handle_wake_lease(wake_req(&tenant, &lease_id)).await;
    let woken = stored(&core, &lease_id).await?;
    assert_eq!(woken.status, LeaseStatus::Running);
    let left = woken.deadline.duration_since(SystemTime::now())?.as_secs();
    assert!((118..=120).contains(&left), "left {left}");
    Ok(())
}

#[tokio::test]
async fn the_suspended_budget_is_capped_at_the_ceiling() -> TestResult {
    let (core, _bus) = core_offering(DormancyCapability::SUSPEND_DISK).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-ceiling").await?;
    let mut req = suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto);
    req.max_suspended_seconds = Some(u32::MAX);
    let SuspendLeaseResponse::Suspended { reap_at, .. } = core.handle_suspend_lease(req).await else {
        return Err("suspend should succeed".into());
    };
    let budget = reap_at.duration_since(SystemTime::now())?.as_secs();
    assert!(
        budget <= u64::from(crate::constants::MAX_SUSPENDED_SECONDS_CEILING),
        "{budget}"
    );
    Ok(())
}

#[tokio::test]
async fn verbs_on_other_states_and_tenants_are_refused() -> TestResult {
    let (core, _bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-owner").await?;
    let other = TenantId::new("rival")?;
    assert!(matches!(
        core.handle_suspend_lease(suspend_req(&other, &lease_id, SuspendRequestLevel::Auto))
            .await,
        SuspendLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    assert!(matches!(
        core.handle_wake_lease(wake_req(&other, &lease_id)).await,
        WakeLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));

    // A lease mid-suspend is not suspended again by a fresh request.
    let mut record = stored(&core, &lease_id).await?;
    let running = record.clone();
    record.status = LeaseStatus::Suspending;
    core.store_lease_update(&running, &record).await?;
    assert!(matches!(
        core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
            .await,
        SuspendLeaseResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    assert!(matches!(
        core.handle_wake_lease(wake_req(&tenant, &lease_id)).await,
        WakeLeaseResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_wake_that_fails_returns_to_suspended_and_the_third_failure_ends_the_lease() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-wake-fails").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;

    for failure in 1..=crate::constants::MAX_WAKE_FAILURES {
        bus.script_wake(Ok(SandboxWakeResponse::Denied(LeaseError::Backend {
            message: "restore failed".into(),
        })))?;
        let response = core.handle_wake_lease(wake_req(&tenant, &lease_id)).await;
        assert!(matches!(
            response,
            WakeLeaseResponse::Denied(LeaseError::Backend { .. })
        ));
        let record = stored(&core, &lease_id).await?;
        assert_eq!(
            record.dormancy.as_ref().ok_or("dormancy recorded")?.wake_failures,
            failure
        );
        let expected = if failure < crate::constants::MAX_WAKE_FAILURES {
            LeaseStatus::Suspended
        } else {
            LeaseStatus::Failed
        };
        assert_eq!(record.status, expected, "after failure {failure}");
    }
    Ok(())
}

#[tokio::test]
async fn a_lost_artifact_fails_the_lease_at_once() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_DISK).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-artifact-lost").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    bus.script_wake(Ok(SandboxWakeResponse::Denied(LeaseError::SuspendArtifactLost {
        lease_id: lease_id.clone(),
    })))?;
    let response = core.handle_wake_lease(wake_req(&tenant, &lease_id)).await;
    assert!(matches!(
        response,
        WakeLeaseResponse::Denied(LeaseError::SuspendArtifactLost { .. })
    ));
    assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Failed);
    Ok(())
}

#[tokio::test]
async fn a_second_wake_while_one_runs_is_told_to_retry() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-single-flight").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    let suspended = stored(&core, &lease_id).await?;
    let mut waking = suspended.clone();
    waking.status = LeaseStatus::Waking;
    core.store_lease_update(&suspended, &waking).await?;

    let response = core.handle_wake_lease(wake_req(&tenant, &lease_id)).await;
    assert!(matches!(
        response,
        WakeLeaseResponse::Denied(LeaseError::LeaseWaking {
            retry_after_ms: 1000,
            ..
        })
    ));
    assert_eq!(bus.asked("sandbox.provider.mock.wake.")?, 0);

    // The operation driver's re-drive is the one caller that may finish it.
    let response = core.wake_lease_inline(wake_req(&tenant, &lease_id), true).await;
    assert!(matches!(response, WakeLeaseResponse::Woken { .. }));
    assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn a_suspend_with_no_reply_stays_suspending_until_it_is_re_driven() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-lost-reply").await?;

    bus.script_suspend(Err(()))?;
    let response = core
        .suspend_lease_inline(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto), false)
        .await;
    assert!(matches!(
        response,
        SuspendLeaseResponse::Denied(LeaseError::Backend { .. })
    ));
    assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Suspending);

    let response = core
        .suspend_lease_inline(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto), true)
        .await;
    assert!(matches!(response, SuspendLeaseResponse::Suspended { .. }));
    let record = stored(&core, &lease_id).await?;
    assert_eq!(record.status, LeaseStatus::Suspended);
    // The re-drive reuses the generation the first attempt sent.
    assert_eq!(record.dormancy.ok_or("dormancy recorded")?.generation, 1);
    Ok(())
}

fn read_req(lease_id: &LeaseId) -> ReadFileRequest {
    ReadFileRequest {
        lease_id: lease_id.clone(),
        path: "/etc/hostname".into(),
        as_user: None,
    }
}

async fn settle_to_running(core: &BrokerCore, lease_id: &LeaseId) -> TestResult {
    for _ in 0..200 {
        if stored(core, lease_id).await?.status == LeaseStatus::Running {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!("the wake did not finish: {:?}", stored(core, lease_id).await?.status).into())
}

async fn assert_wake_on_access(core: BrokerCore, bus: Arc<DormancyBus>) -> TestResult {
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "wake-on-access").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Suspended);

    let core = Arc::new(core);
    core.bind_self();

    // Two verbs race the same suspended lease: both are told to retry,
    // and the worker is asked to wake the sandbox once.
    let first = core.handle_read_file(read_req(&lease_id)).await;
    let second = core.handle_read_file(read_req(&lease_id)).await;
    for response in [first, second] {
        assert!(
            matches!(
                response,
                ReadFileResponse::Denied(LeaseError::LeaseWaking {
                    retry_after_ms: 1000,
                    ..
                })
            ),
            "{response:?}"
        );
    }
    settle_to_running(&core, &lease_id).await?;
    assert_eq!(bus.asked("sandbox.provider.mock.wake.")?, 1);
    Ok(())
}

#[tokio::test]
async fn a_verb_on_a_suspended_lease_starts_one_wake_and_answers_waking() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    assert_wake_on_access(core, bus).await?;
    Ok(())
}

#[tokio::test]
async fn wake_on_access_is_one_durable_operation_when_the_store_exists() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let store = op_store();
    let core = core.with_operations(Arc::new(crate::operations::Operations::new(store.clone(), "broker-a")));
    assert_wake_on_access(core, bus).await?;
    let operations = crate::operations::Operations::new(store, "reader");
    assert!(operations.list_active().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_verb_on_a_waking_lease_answers_waking_without_starting_another_wake() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "wake-in-flight").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    let suspended = stored(&core, &lease_id).await?;
    let mut waking = suspended.clone();
    waking.status = LeaseStatus::Waking;
    core.store_lease_update(&suspended, &waking).await?;
    let core = Arc::new(core);
    core.bind_self();

    let response = core.handle_read_file(read_req(&lease_id)).await;
    assert!(matches!(
        response,
        ReadFileResponse::Denied(LeaseError::LeaseWaking {
            retry_after_ms: 1000,
            ..
        })
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(bus.asked("sandbox.provider.mock.wake.")?, 0);
    assert_eq!(stored(&core, &lease_id).await?.status, LeaseStatus::Waking);
    Ok(())
}

#[tokio::test]
async fn a_verb_on_a_suspended_lease_without_a_self_handle_is_not_running() -> TestResult {
    let (core, bus) = core_offering(DormancyCapability::SUSPEND_MEMORY).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "no-handle").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    let response = core.handle_read_file(read_req(&lease_id)).await;
    assert!(matches!(
        response,
        ReadFileResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    assert_eq!(bus.asked("sandbox.provider.mock.wake.")?, 0);
    Ok(())
}

#[tokio::test]
async fn usage_reports_the_suspended_time_apart_from_running_time() -> TestResult {
    let (core, _bus) = core_offering(DormancyCapability::SUSPEND_DISK).await?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "dormant-usage").await?;
    core.handle_suspend_lease(suspend_req(&tenant, &lease_id, SuspendRequestLevel::Auto))
        .await;
    let entry = core.usage.get(&tenant, &lease_id).await?.ok_or("usage is tracked")?;
    let usage = entry.to_usage(LeaseStatus::Suspended, SystemTime::now() + Duration::from_secs(10));
    assert!(usage.suspended_seconds >= 9, "{usage:?}");
    assert_eq!(usage.suspend_artifact_bytes, Some(4096));
    Ok(())
}
