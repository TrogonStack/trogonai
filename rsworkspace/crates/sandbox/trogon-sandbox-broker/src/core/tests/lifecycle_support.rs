use super::*;

use serde::Serialize;

fn json_reply<T: Serialize>(value: &T) -> anyhow::Result<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(value)?))
}

pub(super) struct UsageBus {
    pub(super) gauges: ResourceUsageResponse,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for UsageBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains(".run_command.") {
            json_reply(&RunCommandResponse::Completed(trogon_sandbox_contract::CommandOutput {
                stdout: vec![b'a'; 3],
                stderr: vec![b'b'; 2],
                ..Default::default()
            }))
        } else if subject.contains(".write_files.") {
            json_reply(&WriteFilesResponse::Written)
        } else if subject.contains(".read_file.") {
            json_reply(&ReadFileResponse::Content { data: vec![0; 7] })
        } else if subject.contains(".resource_usage.") {
            json_reply(&self.gauges)
        } else {
            json_reply(&SandboxFreezeResponse::Done)
        }
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn usage_core(
    registry: Arc<InMemoryWorkerRegistry>,
    gauges: ResourceUsageResponse,
    usage: Arc<dyn LeaseUsageStore>,
    active: Arc<dyn ActiveLeaseStore>,
) -> anyhow::Result<BrokerCore> {
    let mut core = CoreParts {
        worker_registry: registry,
        ..CoreParts::new(Arc::new(UsageBus { gauges }))
    }
    .build()?;
    core.active_leases = active;
    Ok(core.with_usage_store(usage))
}

pub(super) async fn gauge_registry(reports: bool) -> anyhow::Result<Arc<InMemoryWorkerRegistry>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.resources.reports_usage_gauges = CapabilitySupport::from(reports);
    registry
        .inject(worker_heartbeat("mock-w1", ProviderKind::Mock, profile)?)
        .await;
    Ok(registry)
}

pub(super) fn some_gauges() -> ResourceUsageResponse {
    ResourceUsageResponse::Gauges(ResourceGauges {
        cpu_nanos_total: Some(42),
        memory_bytes: Some(1024),
        ..Default::default()
    })
}

pub(super) async fn run_one(core: &BrokerCore, lease_id: &LeaseId) {
    let response = core
        .handle_run_command(RunCommandRequest {
            require_launch_cancellation: Default::default(),
            launch_key: None,
            lease_id: lease_id.clone(),
            command: vec!["true".to_string()],
            cwd: None,
            env: Default::default(),
            timeout_seconds: RunCommandRequest::DEFAULT_TIMEOUT_SECONDS,
            stream_output: Default::default(),
            run_as_user: None,
            output_retention: Default::default(),
            max_output_bytes: None,
            redact_values: Default::default(),
            execution: Default::default(),
        })
        .await;
    assert!(matches!(response, RunCommandResponse::Completed(_)));
}

pub(super) fn usage_of(response: GetLeaseUsageResponse) -> anyhow::Result<trogon_sandbox_contract::LeaseUsage> {
    match response {
        GetLeaseUsageResponse::Usage { usage } => Ok(usage),
        other => anyhow::bail!("expected usage, got {other:?}"),
    }
}

pub(super) fn freeze_reply(response: SandboxFreezeResponse) -> anyhow::Result<Bytes> {
    json_reply(&response)
}

pub(super) async fn running_lease_in(core: &BrokerCore, tenant: &TenantId, id: &str) -> anyhow::Result<LeaseId> {
    let lease_id = LeaseId::new(id);
    core.active_leases
        .put(&running_source_record(tenant, &lease_id)?)
        .await?;
    Ok(lease_id)
}

/// Answers every freeze with `reply`, remembering each subject and whether
/// the lease record already carried a freeze marker when it was asked.
pub(super) struct FreezeProbeBus {
    pub(super) reply: Bytes,
    pub(super) active: Arc<dyn ActiveLeaseStore>,
    pub(super) subjects: std::sync::Mutex<Vec<String>>,
    pub(super) saw_marker: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for FreezeProbeBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        locked(&self.subjects)?.push(subject.to_string());
        let freeze: SandboxFreezeRequest = serde_json::from_slice(&payload)?;
        let marked = self
            .active
            .get(&freeze.lease_id)
            .await?
            .is_some_and(|r| r.freeze_in_flight.is_some());
        self.saw_marker.store(marked, Ordering::SeqCst);
        Ok(self.reply.clone())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn freeze_probe_core(reply: SandboxFreezeResponse) -> anyhow::Result<(BrokerCore, Arc<FreezeProbeBus>)> {
    let mut core = make_core(freeze_reply(SandboxFreezeResponse::Done)?)?;
    let bus = Arc::new(FreezeProbeBus {
        reply: freeze_reply(reply)?,
        active: Arc::clone(&core.active_leases),
        subjects: std::sync::Mutex::new(Vec::new()),
        saw_marker: std::sync::atomic::AtomicBool::new(false),
    });
    core.bus = bus.clone();
    Ok((core, bus))
}

pub(super) async fn lease_with_stale_freeze(
    core: &BrokerCore,
    id: &str,
    status: LeaseStatus,
    thaw: bool,
) -> anyhow::Result<LeaseId> {
    let lease_id = LeaseId::new(id);
    let mut record = running_source_record(&TenantId::new("acme")?, &lease_id)?;
    record.status = status;
    record.freeze_in_flight = Some(FreezeInFlight {
        action: FreezeAction::from(thaw),
        started_at: SystemTime::now() - FREEZE_IN_FLIGHT_STALE_AFTER - Duration::from_secs(1),
        settle_attempts: 0,
        next_settle_at: None,
    });
    core.active_leases.put(&record).await?;
    Ok(lease_id)
}

/// Delegates to an in-memory tag store, failing the first `failures`
/// clears with a backend error.
pub(super) struct FlakyTagStore {
    pub(super) inner: Arc<dyn crate::snapshot_tag::SnapshotTagStore>,
    pub(super) failures: AtomicU32,
}

#[async_trait::async_trait]
impl crate::snapshot_tag::SnapshotTagStore for FlakyTagStore {
    async fn get(&self, tag: &SnapshotTag) -> Result<Option<SnapshotTagRecord>, SnapshotTagStoreError> {
        self.inner.get(tag).await
    }

    async fn get_entry(&self, tag: &SnapshotTag) -> Result<Option<(SnapshotTagRecord, u64)>, SnapshotTagStoreError> {
        self.inner.get_entry(tag).await
    }

    async fn set(
        &self,
        tag: &SnapshotTag,
        record: SnapshotTagRecord,
        expected_revision: Option<u64>,
    ) -> Result<u64, SnapshotTagStoreError> {
        self.inner.set(tag, record, expected_revision).await
    }

    async fn clear(&self, tag: &SnapshotTag, expected_revision: u64) -> Result<(), SnapshotTagStoreError> {
        if self
            .failures
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(SnapshotTagStoreError::Backend("injected failure".into()));
        }
        self.inner.clear(tag, expected_revision).await
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<(SnapshotTag, SnapshotTagRecord)>, SnapshotTagStoreError> {
        self.inner.list(tenant).await
    }
}

/// A core holding one running lease named "build" whose snapshot tag is
/// set, behind a tag store that fails its first `failures` clears.
pub(super) async fn named_lease_with_flaky_tags(failures: u32) -> anyhow::Result<(BrokerCore, TenantId, LeaseId)> {
    let mut core = make_core(mock_provider_response()?)?;
    core.tag_store = Arc::new(FlakyTagStore {
        inner: default_tag_store(),
        failures: AtomicU32::new(0),
    });
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("named-delete");
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.name = Some(LeaseName::new("build")?);
    core.active_leases.put(&record).await?;
    core.named_leases.set(&tenant, "build", &lease_id).await?;
    let digest = Digest::new(format!("sha256:{}", "e".repeat(64)))?;
    let set = core
        .handle_snapshot_tag_set(SetSnapshotTagRequest {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("build")?,
            digest,
        })
        .await;
    assert_eq!(set, SetSnapshotTagResponse::Applied);
    core.tag_store = Arc::new(FlakyTagStore {
        inner: Arc::clone(&core.tag_store),
        failures: AtomicU32::new(failures),
    });
    Ok((core, tenant, lease_id))
}

/// Records everything published and answers no request.
pub(super) struct RecordingBus {
    pub(super) published: std::sync::Mutex<Vec<(String, Bytes)>>,
    pub(super) msg_ids: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for RecordingBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        anyhow::bail!("unexpected request on {subject}")
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        locked(&self.published)?.push((subject.to_string(), payload));
        Ok(())
    }

    async fn publish_with_msg_id(&self, subject: &str, msg_id: &str, payload: Bytes) -> anyhow::Result<()> {
        locked(&self.msg_ids)?.push(msg_id.to_string());
        self.publish(subject, payload).await
    }
}

pub(super) fn recording_bus() -> Arc<RecordingBus> {
    Arc::new(RecordingBus {
        published: std::sync::Mutex::new(Vec::new()),
        msg_ids: std::sync::Mutex::new(Vec::new()),
    })
}

pub(super) async fn retried_delete_publishes(release_announced: Option<bool>) -> anyhow::Result<usize> {
    let mut core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("half-released");
    let record = running_source_record(&tenant, &lease_id)?;
    let bus = recording_bus();
    core.bus = bus.clone();
    // The release removed the active record and then stopped.
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            name: None,
            tag_revision: None,
            release_announced,
            pending_refund: None,
            drive_claims: None,
            announcement: Some(announcement(&record, LeaseReleasedReason::Cancelled)),
            recorded_at: SystemTime::now(),
        })
        .await?;

    let response = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant,
            lease_id: lease_id.clone(),
        })
        .await;
    assert_eq!(response, DeleteLeaseResponse::Deleted { operation_id: None });
    assert!(core.pending_purges.get(&lease_id).await?.is_none());
    let published = locked(&bus.published)?;
    Ok(published
        .iter()
        .filter(|(subject, _)| subject == &subjects::lease_released(&lease_id))
        .count())
}

/// A tenant with one legacy lease holding `cpu`/`wall` seconds of quota.
pub(super) async fn tenant_holding_quota(
    core: &BrokerCore,
    tenant: &TenantId,
    cpu: u32,
    wall: u32,
) -> anyhow::Result<()> {
    core.quota.check_and_reserve(tenant, cpu, wall, 1_000, 1_000).await?;
    core.quota.increment_active_leases(tenant).await?;
    Ok(())
}

pub(super) async fn quota_held(core: &BrokerCore, tenant: &TenantId) -> anyhow::Result<(u32, u32, u32)> {
    let record = core.quota.quota_record(tenant).await?.context("quota record")?;
    Ok((
        record.cpu_seconds_used,
        record.wall_seconds_used,
        core.quota.active_lease_count(tenant).await?,
    ))
}

/// The broker stopped after the release removed the active record and
/// before it refunded the tenant.
pub(super) async fn release_stopped_before_its_refund() -> anyhow::Result<(BrokerCore, TenantId, LeaseId)> {
    let mut core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("unrefunded");
    core.bus = recording_bus();
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            name: None,
            tag_revision: None,
            release_announced: Some(false),
            pending_refund: Some(PendingRefund {
                cpu_seconds: 30,
                wall_seconds: 40,
            }),
            drive_claims: None,
            announcement: None,
            recorded_at: SystemTime::now(),
        })
        .await?;
    Ok((core, tenant, lease_id))
}

/// A running lease holding 30 cpu and 40 wall seconds of its tenant's
/// quota.
pub(super) async fn lease_holding_quota(core: &BrokerCore, name: &str) -> anyhow::Result<(TenantId, LeaseRecord)> {
    let tenant = TenantId::new("acme")?;
    let mut record = running_source_record(&tenant, &LeaseId::new(name))?;
    record.cpu_seconds_consumed = 30;
    record.wall_seconds_consumed = 40;
    core.active_leases.put(&record).await?;
    tenant_holding_quota(core, &tenant, 30, 40).await?;
    Ok((tenant, record))
}

/// The broker removed the active record of a plain release and stopped
/// before refunding it.
pub(super) async fn plain_release_stopped_before_its_refund() -> anyhow::Result<(BrokerCore, TenantId, LeaseId)> {
    let mut core = make_core(mock_provider_response()?)?;
    let bus = recording_bus();
    core.bus = bus;
    let (tenant, record) = lease_holding_quota(&core, "plain-unrefunded").await?;
    core.pending_releases
        .create(&PendingRelease {
            quota_reservation: None,
            lease_id: record.lease_id.clone(),
            tenant: tenant.clone(),
            name: None,
            refund: PendingRefund {
                cpu_seconds: 30,
                wall_seconds: 40,
            },
            drive_claims: Vec::new(),
            announcement: announcement(&record, LeaseReleasedReason::Cancelled),
            recorded_at: SystemTime::now() - PENDING_PURGE_RECONCILE_AFTER * 2,
        })
        .await?;
    core.active_leases.delete(&record.lease_id).await?;
    Ok((core, tenant, record.lease_id))
}

pub(super) async fn build_tag_exists(core: &BrokerCore, tenant: &TenantId) -> anyhow::Result<bool> {
    let tag = SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new("build")?,
    };
    Ok(core.tag_store.get(&tag).await?.is_some())
}

/// Answers freeze requests itself and relays everything else.
pub(super) struct PauseAwareBus {
    pub(super) inner: CapturingBus,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for PauseAwareBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains(".pause.") {
            return freeze_reply(SandboxFreezeResponse::Done);
        }
        self.inner.request(subject, payload).await
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.inner.publish(subject, payload).await
    }
}

pub(super) fn idle_policy(
    stages: &[(u32, trogon_sandbox_contract::IdleAction)],
) -> anyhow::Result<trogon_sandbox_contract::IdlePolicy> {
    let stages = stages
        .iter()
        .map(|&(after_seconds, action)| {
            Ok(trogon_sandbox_contract::IdleStage {
                after_seconds: NonZeroU32::new(after_seconds).context("zero idle stage")?,
                action,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(trogon_sandbox_contract::IdlePolicy::new(stages)?)
}
