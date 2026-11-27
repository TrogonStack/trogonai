use super::*;

pub(super) struct MockBus {
    pub(super) provider_response: Bytes,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for MockBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(self.provider_response.clone())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A bus that records the last dispatch payload it was asked to relay.
pub(super) struct CapturingBus {
    pub(super) provider_response: Bytes,
    pub(super) last_payload: std::sync::Mutex<Option<Bytes>>,
    pub(super) published: std::sync::Mutex<Vec<(String, Bytes)>>,
    pub(super) max_payload: Option<usize>,
}

pub(super) struct FaultingDispatchBus {
    pub(super) response: Option<Bytes>,
    pub(super) cleanup_mode: AtomicU32,
    pub(super) fail_release_publish: std::sync::atomic::AtomicBool,
    pub(super) released_events: AtomicU32,
    pub(super) last_release: std::sync::Mutex<Option<LeaseReleased>>,
    pub(super) dispatches: AtomicU32,
    pub(super) observed_pending: std::sync::atomic::AtomicBool,
    pub(super) active: Arc<dyn ActiveLeaseStore>,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for FaultingDispatchBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if let Ok(cleanup) = serde_json::from_slice::<trogon_sandbox_contract::ProviderCleanupRequest>(&payload) {
            let mode = self.cleanup_mode.load(Ordering::SeqCst);
            anyhow::ensure!(mode != 0, "injected cleanup timeout");
            let response = trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
                identity: cleanup.identity,
                intent: if mode == 2 {
                    CancelIntent::Release
                } else {
                    cleanup.intent
                },
            };
            return Ok(serde_json::to_vec(&response)?.into());
        }
        let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        self.observed_pending.store(
            self.active
                .get(&dispatch.lease_id)
                .await?
                .is_some_and(|record| record.status == LeaseStatus::Pending),
            Ordering::SeqCst,
        );
        self.response
            .clone()
            .ok_or_else(|| anyhow::anyhow!("injected dispatch timeout"))
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        if subject.starts_with("sandbox.lease.released.") {
            anyhow::ensure!(
                !self.fail_release_publish.load(Ordering::SeqCst),
                "injected release publication failure"
            );
            self.released_events.fetch_add(1, Ordering::SeqCst);
            *locked(&self.last_release)? = Some(serde_json::from_slice(&payload)?);
        }
        Ok(())
    }
}

pub(super) async fn native_failure_fixture(
    response: Option<Bytes>,
) -> anyhow::Result<(BrokerCore, Arc<FaultingDispatchBus>, DriveId, LeaseRequest)> {
    let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let bus = Arc::new(FaultingDispatchBus {
        response,
        cleanup_mode: AtomicU32::new(0),
        fail_release_publish: Default::default(),
        released_events: AtomicU32::new(0),
        last_release: Default::default(),
        dispatches: AtomicU32::new(0),
        observed_pending: std::sync::atomic::AtomicBool::new(false),
        active: active.clone(),
    });
    let mut core = make_core(mock_provider_response()?)?;
    core.active_leases = active;
    core.bus = bus.clone();
    let tenant = TenantId::new("test-tenant")?;
    let drive = DriveId {
        tenant: tenant.clone(),
        name: trogon_sandbox_contract::DriveName::new("native-recovery")?,
    };
    let volume = trogon_sandbox_contract::ProviderVolumeRef {
        provider: ProviderKind::Fly,
        volume_id: trogon_sandbox_contract::ProviderVolumeId::new("vol_recovery")?,
    };
    let response = core
        .handle_create_drive(CreateDriveRequest {
            tenant,
            drive_id: drive.clone(),
            size_gib: Some(10),
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: volume.clone(),
            },
        })
        .await;
    assert!(matches!(response, CreateDriveResponse::Created(_)), "{response:?}");
    let mount = DriveMount {
        drive_id: drive.clone(),
        mount_path: MountPath::new("/data")?,
        mode: DriveMountMode::ReadWrite,
    };
    let request = edit_request_spec(make_request(vec![ProviderKind::Fly])?, |parts| {
        parts.drives = vec![mount.clone()];
    })?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut heartbeat = worker_heartbeat(
        "native-recovery-worker",
        ProviderKind::Fly,
        default_capability_profile(),
    )?;
    heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
    heartbeat.native_volume_bindings = vec![trogon_sandbox_contract::NativeVolumeBinding { mount, storage: volume }];
    registry.inject(heartbeat).await;
    core.worker_registry = registry;
    Ok((core, bus, drive, request))
}

pub(super) async fn assert_native_dispatch_remains_recoverable(response: Option<Bytes>) -> anyhow::Result<()> {
    let (core, bus, drive, request) = native_failure_fixture(response).await?;
    let first = core.handle_lease_request(request.clone()).await;
    assert!(
        matches!(first, LeaseResponse::Denied(LeaseError::Backend { .. })),
        "{first:?}"
    );
    assert!(
        bus.observed_pending.load(Ordering::SeqCst),
        "dispatch must follow a durable pending lease record"
    );
    let records = core.active_leases.all().await?;
    assert_eq!(records.len(), 1);
    let record = records.first().context("one lease record")?;
    assert_eq!(record.status, LeaseStatus::Pending);
    assert_eq!(
        core.drive_store
            .get(&drive)
            .await?
            .context("drive record")?
            .attached_lease,
        Some(record.lease_id.clone())
    );
    assert_eq!(core.quota.active_lease_count(&request.tenant).await?, 1);
    let retry = core.handle_lease_request(request).await;
    assert!(
        matches!(retry, LeaseResponse::Denied(_)),
        "unknown dispatch cannot be reported as a grant"
    );
    assert_eq!(bus.dispatches.load(Ordering::SeqCst), 1);
    Ok(())
}

/// Emulates a targeted worker's attempt ledger: the first send launches and
/// records the receipt, repeats return the stored receipt. The first reply
/// is lost.
pub(super) struct LostReplyWorkerBus {
    pub(super) launched: std::sync::Mutex<std::collections::HashMap<String, Bytes>>,
    pub(super) sends: AtomicU32,
    pub(super) lose_first_reply: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for LostReplyWorkerBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
        let target = dispatch.target.context("targeted dispatch")?;
        self.sends.fetch_add(1, Ordering::SeqCst);
        let receipt = Bytes::from(serde_json::to_vec(&CoreProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new("machine-1")?,
            evidence_subject: "e".into(),
            worker_id: Some(target.worker_id.clone()),
            attempt_id: Some(target.attempt_id.clone()),
        })?);
        let reply = locked(&self.launched)?
            .entry(format!("{:?}", target.attempt_id))
            .or_insert(receipt)
            .clone();
        if self.lose_first_reply.swap(false, Ordering::SeqCst) {
            anyhow::bail!("reply lost");
        }
        Ok(reply)
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Loses the first dispatch reply, then refuses the re-send the way a
/// worker whose launch failed does. Cleanup requests are left unanswered.
pub(super) struct RefusedRedriveBus {
    pub(super) dispatches: AtomicU32,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for RefusedRedriveBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if serde_json::from_slice::<trogon_sandbox_contract::ProviderDispatchPayload>(&payload).is_err() {
            anyhow::bail!("cleanup not answered");
        }
        if self.dispatches.fetch_add(1, Ordering::SeqCst) == 0 {
            anyhow::bail!("reply lost");
        }
        Ok(serde_json::to_vec(&ProviderDispatchReply::LaunchFailed {
            provider_error: "image unavailable".into(),
        })?
        .into())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) struct GatedReceiptBus {
    pub(super) entered: tokio::sync::Notify,
    pub(super) resume: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for GatedReceiptBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if let Ok(cleanup) = serde_json::from_slice::<trogon_sandbox_contract::ProviderCleanupRequest>(&payload) {
            return Ok(
                serde_json::to_vec(&trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
                    identity: cleanup.identity,
                    intent: cleanup.intent,
                })?
                .into(),
            );
        }
        let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
        let target = dispatch.target.context("targeted dispatch")?;
        self.entered.notify_one();
        self.resume.notified().await;
        let receipt = CoreProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new("allocated-before-cleanup")?,
            evidence_subject: "fixture.evidence".into(),
            worker_id: Some(target.worker_id),
            attempt_id: Some(target.attempt_id),
        };
        Ok(serde_json::to_vec(&receipt)?.into())
    }
    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Answers each per-lease verb with a fixed, recognisable reply.
impl CapturingBus {
    pub(super) fn new(provider_response: Bytes) -> Self {
        Self {
            provider_response,
            last_payload: std::sync::Mutex::new(None),
            max_payload: None,
            published: Default::default(),
        }
    }

    pub(super) fn limited_to(mut self, max_payload: usize) -> Self {
        self.max_payload = Some(max_payload);
        self
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for CapturingBus {
    async fn request(&self, _subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        *locked(&self.last_payload)? = Some(payload.clone());
        if let Ok(cleanup) = serde_json::from_slice::<trogon_sandbox_contract::ProviderCleanupRequest>(&payload) {
            return Ok(
                serde_json::to_vec(&trogon_sandbox_contract::ProviderCleanupResponse::Cleaned {
                    identity: cleanup.identity,
                    intent: cleanup.intent,
                })?
                .into(),
            );
        }
        if let Ok(dispatch) = serde_json::from_slice::<trogon_sandbox_contract::ProviderDispatchPayload>(&payload)
            && let Some(target) = dispatch.target
        {
            let mut receipt: CoreProviderLease = serde_json::from_slice(&self.provider_response)?;
            receipt.worker_id = Some(target.worker_id);
            receipt.attempt_id = Some(target.attempt_id);
            return Ok(serde_json::to_vec(&receipt)?.into());
        }
        Ok(self.provider_response.clone())
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        locked(&self.published)?.push((subject.to_owned(), payload));
        Ok(())
    }

    fn max_payload(&self) -> Option<usize> {
        self.max_payload
    }
}

/// A quota store that counts `check_and_reserve` calls.
pub(super) struct RoutingBus {
    pub(super) checkpoint_response: Bytes,
    pub(super) dispatch_response: Bytes,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for RoutingBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains("checkpoint_running") {
            Ok(self.checkpoint_response.clone())
        } else {
            Ok(self.dispatch_response.clone())
        }
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}
