use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn an_orphaned_drive_claim_past_grace_is_released() -> TestResult {
    let (core, store, _, drive, lease_id) = orphan_claim_fixture().await?;
    store.backdate_claim(&drive, DEFAULT_ORPHAN_CLAIM_GRACE * 2);
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, None);
    // A later claim by another lease is untouched.
    let other = LeaseId::new("other");
    store.set_attached_lease(&drive, Some(other.clone())).await?;
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, Some(other));
    drop(lease_id);
    Ok(())
}

#[tokio::test]
async fn an_orphan_past_the_first_scan_page_is_released_within_bounded_ticks() -> TestResult {
    let (core, store, tenant, _, _) = orphan_claim_fixture().await?;
    let total = ORPHAN_CLAIM_SCAN_LIMIT + 76;
    let mut last = None;
    for i in 0..total {
        let drive = make_drive_id(&tenant, &format!("bulk-{i:05}"))?;
        store.create(&tenant, &drive, 1, "bucket".into()).await?;
        last = Some(drive);
    }
    // The drive that sorts last, so it lies beyond the first page.
    let drive = last.ok_or("no drives created")?;
    let lease_id = LeaseId::new("late-orphan");
    store.set_attached_lease(&drive, Some(lease_id.clone())).await?;
    store.backdate_claim(&drive, DEFAULT_ORPHAN_CLAIM_GRACE * 2);
    let first = store.list_claims(None, ORPHAN_CLAIM_SCAN_LIMIT).await?;
    assert!(first.claims.iter().all(|c| c.drive_id != drive));
    assert!(first.next.is_some());

    let ticks = total.div_ceil(ORPHAN_CLAIM_SCAN_LIMIT) + 1;
    for _ in 0..ticks {
        core.reconcile_orphan_drive_claims().await;
    }
    assert_eq!(claim_holder(&store, &drive).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_claim_whose_grant_is_still_inside_grace_is_kept() -> TestResult {
    let (core, store, _, drive, lease_id) = orphan_claim_fixture().await?;
    // The claim is set and the active record is not written yet.
    store.backdate_claim(&drive, DEFAULT_ORPHAN_CLAIM_GRACE / 2);
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, Some(lease_id));
    Ok(())
}

#[tokio::test]
async fn a_claim_with_an_active_lease_is_kept() -> TestResult {
    let (core, store, tenant, drive, lease_id) = orphan_claim_fixture().await?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    store.backdate_claim(&drive, DEFAULT_ORPHAN_CLAIM_GRACE * 2);
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, Some(lease_id));
    Ok(())
}

#[tokio::test]
async fn a_claim_with_a_pending_release_or_purge_marker_is_kept() -> TestResult {
    let (core, store, tenant, drive, lease_id) = orphan_claim_fixture().await?;
    store.backdate_claim(&drive, DEFAULT_ORPHAN_CLAIM_GRACE * 2);
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant,
            name: None,
            tag_revision: None,
            release_announced: Some(true),
            pending_refund: None,
            drive_claims: None,
            announcement: None,
            recorded_at: SystemTime::now(),
        })
        .await?;
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, Some(lease_id));
    Ok(())
}

#[tokio::test]
async fn a_claim_rewritten_after_the_listing_is_skipped_without_error() -> TestResult {
    let (_core, store, _, drive, lease_id) = orphan_claim_fixture().await?;
    store.backdate_claim(&drive, DEFAULT_ORPHAN_CLAIM_GRACE * 2);
    let claims = store.list_claims(None, 10).await?.claims;
    store.touch_revision(&drive);
    assert!(!store.release_claim_at(&drive, &lease_id, claims[0].revision).await?);
    assert_eq!(claim_holder(&store, &drive).await?, Some(lease_id));
    Ok(())
}

#[tokio::test]
async fn a_claim_without_a_write_time_ages_from_first_sighting() -> TestResult {
    struct Undated(Arc<InMemoryDriveStore>);
    #[async_trait::async_trait]
    impl DriveStore for Undated {
        async fn create(
            &self,
            tenant: &TenantId,
            drive: &DriveId,
            size: u64,
            bucket: String,
        ) -> Result<trogon_sandbox_contract::DriveRecord, DriveStoreError> {
            self.0.create(tenant, drive, size, bucket).await
        }
        async fn get(&self, drive: &DriveId) -> Result<Option<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
            self.0.get(drive).await
        }
        async fn list(&self, tenant: &TenantId) -> Result<Vec<trogon_sandbox_contract::DriveRecord>, DriveStoreError> {
            self.0.list(tenant).await
        }
        async fn delete(&self, drive: &DriveId) -> Result<(), DriveStoreError> {
            self.0.delete(drive).await
        }
        async fn set_attached_lease(&self, drive: &DriveId, lease: Option<LeaseId>) -> Result<(), DriveStoreError> {
            self.0.set_attached_lease(drive, lease).await
        }
        async fn list_claims(&self, after: Option<&str>, limit: usize) -> Result<ClaimPage, DriveStoreError> {
            let mut page = self.0.list_claims(after, limit).await?;
            for claim in &mut page.claims {
                claim.claimed_at = None;
            }
            Ok(page)
        }
        async fn release_claim_at(
            &self,
            drive: &DriveId,
            lease: &LeaseId,
            revision: u64,
        ) -> Result<bool, DriveStoreError> {
            self.0.release_claim_at(drive, lease, revision).await
        }
    }
    let (mut core, store, _, drive, lease_id) = orphan_claim_fixture().await?;
    core.drive_store = Arc::new(Undated(store.clone()));
    core.orphan_claim_grace = Duration::from_millis(1);
    core.config.provider_dispatch_timeout = Duration::ZERO;
    // First sighting only starts the clock.
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, Some(lease_id));
    tokio::time::sleep(Duration::from_millis(20)).await;
    core.reconcile_orphan_drive_claims().await;
    assert_eq!(claim_holder(&store, &drive).await?, None);
    Ok(())
}

#[tokio::test]
async fn another_broker_repairs_a_late_drive_claim_after_admitting_broker_crashes() -> TestResult {
    let (mut admitting, bus, drive, request) = native_failure_fixture(None).await?;
    let gated = Arc::new(GatedDriveClaim {
        inner: admitting.drive_store.clone(),
        entered: Default::default(),
        resume: Default::default(),
        committed: Default::default(),
    });
    admitting.drive_store = gated.clone();
    let mut recovering = make_core(mock_provider_response()?)?;
    recovering.active_leases = admitting.active_leases.clone();
    recovering.idempotency = admitting.idempotency.clone();
    recovering.quota = admitting.quota.clone();
    recovering.drive_store = admitting.drive_store.clone();
    recovering.bus = admitting.bus.clone();
    let tenant = request.tenant.clone();
    let allocating = tokio::spawn(async move { admitting.handle_lease_request(request).await });
    tokio::time::timeout(Duration::from_secs(2), gated.entered.notified()).await?;
    let record = recovering.active_leases.all().await?.pop().ok_or("lease record")?;
    bus.cleanup_mode.store(1, Ordering::SeqCst);
    recovering
        .handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;
    gated.resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), gated.committed.notified()).await?;
    allocating.abort();
    assert!(allocating.await.err().ok_or("allocation finished")?.is_cancelled());
    assert_eq!(
        gated.get(&drive).await?.ok_or("drive record")?.attached_lease,
        Some(record.lease_id)
    );
    recovering.reconcile_expired_leases().await;
    assert!(
        gated.get(&drive).await?.ok_or("drive record")?.attached_lease.is_none(),
        "completed cleanup must remain discoverable after a delayed claim commits and its broker crashes"
    );
    assert_eq!(recovering.quota.active_lease_count(&tenant).await?, 0);
    assert_eq!(bus.dispatches.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn delayed_drive_release_cannot_clear_a_newer_lease_claim() -> TestResult {
    let store = Arc::new(GatedDriveRelease {
        inner: Arc::new(InMemoryDriveStore::new()),
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let tenant = TenantId::new("test-tenant")?;
    let drive = make_drive_id(&tenant, "release-race")?;
    store.inner.create(&tenant, &drive, 1, "bucket".into()).await?;
    let core = Arc::new(make_core_with_drive_store(mock_provider_response()?, store.clone())?);
    let request = make_request(vec![ProviderKind::Mock])?;
    let request = {
        let mut request = request;
        let mut parts = request.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive.clone(),
            mount_path: MountPath::new("/data")?,
            mode: DriveMountMode::ReadWrite,
        }];
        request.spec = SandboxSpec::new(parts)?;
        request
    };
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request).await else {
        return Err("fixture lease denied".into());
    };
    let cancelling = tokio::spawn({
        let core = core.clone();
        async move {
            core.handle_cancel(granted.lease_id, LeaseReleasedReason::Cancelled)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), store.entered.notified()).await?;
    store.inner.set_attached_lease(&drive, None).await?;
    let newer = LeaseId::new("newer-lease");
    store.inner.set_attached_lease(&drive, Some(newer.clone())).await?;
    store.resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), cancelling).await???;
    assert_eq!(
        store.inner.get(&drive).await?.ok_or("drive record")?.attached_lease,
        Some(newer)
    );
    Ok(())
}

#[tokio::test]
async fn native_drive_dispatch_preserves_registered_backend_without_creating_s3_mounts() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_id = make_drive_id(&tenant, "native-data")?;
    let store = default_drive_store();
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let mut core = make_core_with_drive_store(mock_provider_response()?, store.clone())?;
    core.bus = bus.clone();
    let volume = trogon_sandbox_contract::ProviderVolumeRef {
        provider: ProviderKind::Fly,
        volume_id: "vol_example".to_owned().try_into()?,
    };
    assert!(matches!(
        core.handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
            size_gib: Some(10),
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: volume.clone()
            }
        })
        .await,
        CreateDriveResponse::Created(_)
    ));
    let request = make_request(vec![ProviderKind::Mock, ProviderKind::Fly])?;
    let request = {
        let mut request = request;
        let mut parts = request.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive_id.clone(),
            mount_path: MountPath::new("/data")?,
            mode: DriveMountMode::ReadWrite,
        }];
        request.spec = SandboxSpec::new(parts)?;
        request
    };
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    for (worker_id, volume_id) in [("a-incompatible", "vol_other"), ("z-compatible", "vol_example")] {
        let mut heartbeat = worker_heartbeat(
            worker_id,
            ProviderKind::Fly,
            crate::default_capability_profile::default_capability_profile(),
        )?;
        heartbeat.dispatch_protocol = trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1;
        heartbeat.native_volume_bindings = vec![trogon_sandbox_contract::NativeVolumeBinding {
            mount: request.spec.drives()[0].clone(),
            storage: trogon_sandbox_contract::ProviderVolumeRef {
                provider: ProviderKind::Fly,
                volume_id: volume_id.to_owned().try_into()?,
            },
        }];
        registry.inject(heartbeat).await;
    }
    core.worker_registry = registry;
    let response = core.handle_lease_request(request).await;
    let LeaseResponse::Granted(granted) = response else {
        return Err(format!("{response:?}").into());
    };
    assert_eq!(granted.provider, ProviderKind::Fly);
    let payload = locked(&bus.last_payload)?.clone().ok_or("dispatched payload")?;
    let dispatched: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&payload)?;
    assert_eq!(
        dispatched.target.as_ref().ok_or("dispatch target")?.worker_id.as_str(),
        "z-compatible"
    );
    assert!(dispatched.request.spec.mounts().is_empty());
    assert_eq!(dispatched.resolved_drives.len(), 1);
    assert_eq!(dispatched.resolved_drives[0].storage.provider_volume(), Some(&volume));
    assert_eq!(
        dispatched.resolved_drives[0].attached_lease.as_ref(),
        Some(&granted.lease_id)
    );
    assert_eq!(
        store.get(&drive_id).await?.ok_or("drive record")?.attached_lease,
        Some(granted.lease_id)
    );
    Ok(())
}
