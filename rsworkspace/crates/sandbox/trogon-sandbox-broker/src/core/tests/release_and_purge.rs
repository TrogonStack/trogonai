use super::*;

#[tokio::test]
async fn a_lease_being_purged_no_longer_answers_to_its_name() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(0).await?;
    let marker = PendingPurge {
        quota_reservation: None,
        lease_id: lease_id.clone(),
        tenant: tenant.clone(),
        name: Some("build".into()),
        tag_revision: None,
        release_announced: Some(false),
        pending_refund: None,
        drive_claims: None,
        announcement: None,
        recorded_at: SystemTime::now(),
    };
    core.pending_purges.put(&marker).await?;

    let found = core
        .handle_get_lease_by_name(GetLeaseByNameRequest {
            tenant: tenant.clone(),
            name: LeaseName::new("build")?,
        })
        .await;
    assert_eq!(found, GetLeaseByNameResponse::NotFound);

    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.tenant = tenant;
    request.name = Some(LeaseName::new("build")?);
    request.persistent = LeasePersistence::from(false);
    let LeaseResponse::Granted(granted) = core.handle_lease_request(request).await else {
        return Err("a name whose lease is being purged must take a new lease".into());
    };
    assert_ne!(
        granted.lease_id, lease_id,
        "reconnect must not return the purging lease"
    );
    assert!(granted.created.is_created());
    Ok(())
}

#[tokio::test]
async fn a_retried_delete_announces_a_release_that_stopped_before_its_event() -> TestResult {
    assert_eq!(retried_delete_publishes(Some(false)).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_retried_delete_does_not_announce_a_release_already_announced() -> TestResult {
    assert_eq!(retried_delete_publishes(Some(true)).await?, 0);
    assert_eq!(retried_delete_publishes(None).await?, 0);
    Ok(())
}

#[tokio::test]
async fn retried_plain_release_keeps_the_brokers_original_release_time() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("release-time-retry");
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;
    let mut serialized = serde_json::to_value(announcement(&record, LeaseReleasedReason::Completed))?;
    let original_time = serde_json::to_value(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000))?;
    serialized["released_at"] = original_time.clone();
    core.pending_releases
        .create(&PendingRelease {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            name: None,
            refund: PendingRefund {
                cpu_seconds: 0,
                wall_seconds: 0,
            },
            drive_claims: vec![],
            announcement: serde_json::from_value(serialized)?,
            recorded_at: SystemTime::now(),
        })
        .await?;
    let bus = recording_bus();
    core.bus = bus.clone();

    core.handle_cancel(lease_id.clone(), LeaseReleasedReason::Completed)
        .await?;

    let published = locked(&bus.published)?;
    let (_, payload) = published
        .iter()
        .find(|(subject, _)| subject == &subjects::lease_released(&lease_id))
        .ok_or("the release was published")?;
    let event: serde_json::Value = serde_json::from_slice(payload)?;
    assert_eq!(event["released_at"], original_time);
    Ok(())
}

#[tokio::test]
async fn every_lease_released_publish_of_a_lease_carries_one_message_id() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("announced-twice");
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;
    let bus = recording_bus();
    core.bus = bus.clone();

    let deleted = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert_eq!(deleted, DeleteLeaseResponse::Deleted { operation_id: None });
    // The marker write recording the announcement was lost, so the
    // reconciler announces the release again.
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant,
            name: None,
            tag_revision: None,
            release_announced: Some(false),
            pending_refund: None,
            drive_claims: None,
            announcement: Some(announcement(&record, LeaseReleasedReason::Cancelled)),
            recorded_at: SystemTime::now() - PENDING_PURGE_RECONCILE_AFTER * 2,
        })
        .await?;
    core.reconcile_pending_purges().await;

    let ids = locked(&bus.msg_ids)?.clone();
    assert_eq!(
        ids,
        vec![lease_released_msg_id(&lease_id); 2],
        "both publishes must dedupe as one event"
    );
    Ok(())
}

#[tokio::test]
async fn a_retried_delete_refunds_a_release_that_stopped_before_its_refund() -> TestResult {
    let (core, tenant, lease_id) = release_stopped_before_its_refund().await?;
    let response = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id,
        })
        .await;
    assert_eq!(response, DeleteLeaseResponse::Deleted { operation_id: None });
    assert_eq!(quota_held(&core, &tenant).await?, (0, 0, 0));
    Ok(())
}

#[tokio::test]
async fn reconciler_refunds_a_release_that_stopped_before_its_refund() -> TestResult {
    let (core, tenant, lease_id) = release_stopped_before_its_refund().await?;
    let mut marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    marker.recorded_at = SystemTime::now() - PENDING_PURGE_RECONCILE_AFTER * 2;
    core.pending_purges.put(&marker).await?;

    core.reconcile_pending_purges().await;
    assert_eq!(quota_held(&core, &tenant).await?, (0, 0, 0));
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_release_that_refunded_leaves_no_refund_for_the_reconciler() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(PURGE_TAG_DELETE_ATTEMPTS).await?;
    let mut record = core.active_leases.get(&lease_id).await?.context("record")?;
    record.cpu_seconds_consumed = 30;
    record.wall_seconds_consumed = 40;
    core.active_leases.put(&record).await?;
    // Two leases' worth, so a second refund would show.
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    tenant_holding_quota(&core, &tenant, 30, 40).await?;

    // The release refunds, then the purge fails on the tag store.
    let first = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(matches!(first, DeleteLeaseResponse::Denied(_)));
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    let mut marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    assert_eq!(marker.pending_refund, None);

    marker.recorded_at = SystemTime::now() - PENDING_PURGE_RECONCILE_AFTER * 2;
    core.pending_purges.put(&marker).await?;
    core.reconcile_pending_purges().await;
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn a_refund_the_reconciler_already_took_is_not_applied_by_the_release() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(PURGE_TAG_DELETE_ATTEMPTS).await?;
    let mut record = core.active_leases.get(&lease_id).await?.context("record")?;
    record.cpu_seconds_consumed = 30;
    record.wall_seconds_consumed = 40;
    core.active_leases.put(&record).await?;
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    // A marker whose refund another replica already applied.
    let recorded_at = SystemTime::now();
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
            recorded_at,
        })
        .await?;
    assert!(
        core.quota
            .refund_release(&tenant, &lease_id, recorded_at, 30, 40)
            .await?
    );
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));

    core.cancel_lease(lease_id, CancelIntent::Purge).await?;
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn only_one_of_a_release_and_a_redo_applies_the_purge_refund() -> TestResult {
    let (core, tenant, lease_id) = release_stopped_before_its_refund().await?;
    let marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    // The redo takes the refund first; a second redo finds it taken.
    let marker = core.redo_pending_refund(marker).await?;
    core.redo_pending_refund(PendingPurge {
        pending_refund: Some(PendingRefund {
            cpu_seconds: 30,
            wall_seconds: 40,
        }),
        ..marker
    })
    .await?;
    assert_eq!(quota_held(&core, &tenant).await?, (0, 0, 0));
    Ok(())
}

#[tokio::test]
async fn a_refund_applied_before_the_marker_update_is_not_repeated() -> TestResult {
    let (core, tenant, lease_id) = release_stopped_before_its_refund().await?;
    // Two leases' worth, so a second refund would show.
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    let marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    // The broker applied the refund and stopped before clearing the
    // marker's `pending_refund`.
    assert!(
        core.quota
            .refund_release(&tenant, &lease_id, marker.recorded_at, 30, 40)
            .await?
    );
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));

    core.redo_pending_refund(marker).await?;
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn a_refund_redelivered_many_times_applies_once() -> TestResult {
    let (core, tenant, lease_id) = release_stopped_before_its_refund().await?;
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    let marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    for _ in 0..3 {
        core.redo_pending_refund(marker.clone()).await?;
    }
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn concurrent_redos_of_one_refund_apply_once() -> TestResult {
    let (core, tenant, lease_id) = release_stopped_before_its_refund().await?;
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    let marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    let results = futures::future::join_all((0..8).map(|_| core.redo_pending_refund(marker.clone()))).await;
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn a_plain_release_that_stopped_before_its_refund_is_refunded_once_by_the_reconciler() -> TestResult {
    let (core, tenant, lease_id) = plain_release_stopped_before_its_refund().await?;
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));

    core.reconcile_pending_releases().await;
    assert_eq!(quota_held(&core, &tenant).await?, (0, 0, 0));
    assert!(core.pending_releases.all().await?.is_empty());

    // Redelivery: a second pass, and a second marker for the same lease
    // written by a late release, find the receipt.
    tenant_holding_quota(&core, &tenant, 30, 40).await?;
    core.reconcile_pending_releases().await;
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    assert!(!lease_id.as_str().is_empty());
    Ok(())
}

#[tokio::test]
async fn the_redo_of_a_plain_release_clears_the_drive_claim_its_marker_carries() -> TestResult {
    let (core, tenant, lease_id) = plain_release_stopped_before_its_refund().await?;
    let drive = make_drive_id(&tenant, "scratch")?;
    let created = core
        .handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive.clone(),
            size_gib: Some(10),
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: trogon_sandbox_contract::ProviderVolumeRef {
                    provider: ProviderKind::Fly,
                    volume_id: trogon_sandbox_contract::ProviderVolumeId::new("vol_plain")?,
                },
            },
        })
        .await;
    assert!(matches!(created, CreateDriveResponse::Created(_)));
    core.drive_store
        .set_attached_lease(&drive, Some(lease_id.clone()))
        .await?;
    let marker = core.pending_releases.all().await?.remove(0);
    core.pending_releases.clear(&lease_id).await?;
    core.pending_releases
        .create(&PendingRelease {
            drive_claims: vec![drive.clone()],
            ..marker
        })
        .await?;

    core.reconcile_pending_releases().await;
    assert_eq!(
        core.drive_store.get(&drive).await?.context("drive")?.attached_lease,
        None
    );
    assert_eq!(quota_held(&core, &tenant).await?, (0, 0, 0));
    Ok(())
}

#[tokio::test]
async fn a_plain_release_refunds_and_leaves_no_marker() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let (tenant, record) = lease_holding_quota(&core, "plain-release").await?;

    core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)
        .await?;
    assert_eq!(quota_held(&core, &tenant).await?, (0, 0, 0));
    assert!(core.pending_releases.all().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn concurrent_plain_releases_and_a_redo_refund_once() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let (tenant, record) = lease_holding_quota(&core, "plain-concurrent").await?;
    // A second lease's worth, so a second refund would show.
    tenant_holding_quota(&core, &tenant, 30, 40).await?;

    let results = futures::future::join_all(
        (0..6).map(|_| core.handle_cancel(record.lease_id.clone(), LeaseReleasedReason::Cancelled)),
    )
    .await;
    // A release that finds the record already gone reports not found.
    assert!(results.iter().any(Result::is_ok), "{results:?}");
    core.reconcile_pending_releases().await;
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn a_release_that_stopped_before_removing_the_record_leaves_the_lease_alone() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let (tenant, record) = lease_holding_quota(&core, "plain-not-removed").await?;
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

    core.reconcile_pending_releases().await;
    assert!(core.pending_releases.all().await?.is_empty());
    assert!(core.active_leases.get(&record.lease_id).await?.is_some());
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    Ok(())
}

#[tokio::test]
async fn a_young_release_marker_is_left_to_the_release_that_wrote_it() -> TestResult {
    let (core, tenant, lease_id) = plain_release_stopped_before_its_refund().await?;
    let mut marker = core.pending_releases.all().await?.remove(0);
    core.pending_releases.clear(&lease_id).await?;
    marker.recorded_at = SystemTime::now();
    core.pending_releases.create(&marker).await?;

    core.reconcile_pending_releases().await;
    assert_eq!(quota_held(&core, &tenant).await?, (30, 40, 1));
    assert_eq!(core.pending_releases.all().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_redo_of_a_plain_release_announces_it() -> TestResult {
    let (mut core, _, lease_id) = plain_release_stopped_before_its_refund().await?;
    let bus = recording_bus();
    core.bus = bus.clone();
    core.reconcile_pending_releases().await;
    let published = locked(&bus.published)?;
    assert_eq!(
        published
            .iter()
            .filter(|(subject, _)| subject == &subjects::lease_released(&lease_id))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_stopped_delete_releases_the_drive_claim_the_marker_carries() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("claim-carried");
    let drive = make_drive_id(&tenant, "scratch")?;
    let created = core
        .handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive.clone(),
            size_gib: Some(10),
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: trogon_sandbox_contract::ProviderVolumeRef {
                    provider: ProviderKind::Fly,
                    volume_id: trogon_sandbox_contract::ProviderVolumeId::new("vol_carried")?,
                },
            },
        })
        .await;
    assert!(matches!(created, CreateDriveResponse::Created(_)));
    core.drive_store
        .set_attached_lease(&drive, Some(lease_id.clone()))
        .await?;
    // No active record and no readable released record: only the marker
    // knows which drives the lease held.
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            name: None,
            tag_revision: None,
            release_announced: Some(true),
            pending_refund: None,
            drive_claims: Some(vec![drive.clone()]),
            announcement: None,
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
    assert_eq!(
        core.drive_store.get(&drive).await?.context("drive")?.attached_lease,
        None
    );
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_retried_delete_clears_a_drive_claim_the_release_left() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("acme")?;
    let lease_id = LeaseId::new("claim-left");
    let drive = make_drive_id(&tenant, "scratch")?;
    let created = core
        .handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive.clone(),
            size_gib: Some(10),
            storage: trogon_sandbox_contract::DriveStorage::ProviderVolume {
                provider_volume: trogon_sandbox_contract::ProviderVolumeRef {
                    provider: ProviderKind::Fly,
                    volume_id: trogon_sandbox_contract::ProviderVolumeId::new("vol_claim")?,
                },
            },
        })
        .await;
    assert!(matches!(created, CreateDriveResponse::Created(_)));
    core.drive_store
        .set_attached_lease(&drive, Some(lease_id.clone()))
        .await?;
    core.bus = recording_bus();
    // The release removed the active record and stopped before it
    // cleared the drive claim.
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            name: None,
            tag_revision: None,
            release_announced: Some(true),
            pending_refund: None,
            drive_claims: Some(vec![drive.clone()]),
            announcement: None,
            recorded_at: SystemTime::now(),
        })
        .await?;

    let response = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant,
            lease_id,
        })
        .await;
    assert_eq!(response, DeleteLeaseResponse::Deleted { operation_id: None });
    assert_eq!(
        core.drive_store.get(&drive).await?.context("drive")?.attached_lease,
        None
    );
    Ok(())
}

#[tokio::test]
async fn delete_lease_retries_a_failing_tag_store() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(PURGE_TAG_DELETE_ATTEMPTS - 1).await?;
    let deleted = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert_eq!(deleted, DeleteLeaseResponse::Deleted { operation_id: None });
    assert!(!build_tag_exists(&core, &tenant).await?);
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_retried_delete_by_id_finishes_an_unfinished_purge() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(PURGE_TAG_DELETE_ATTEMPTS).await?;
    let first = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(
        matches!(first, DeleteLeaseResponse::Denied(LeaseError::Backend { .. })),
        "{first:?}"
    );
    assert_eq!(core.active_leases.get(&lease_id).await?, None);
    assert!(build_tag_exists(&core, &tenant).await?);
    assert!(core.pending_purges.get(&lease_id).await?.is_some());

    // Another tenant cannot finish (or learn about) the purge.
    let foreign = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: TenantId::new("intruder")?,
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(matches!(
        foreign,
        DeleteLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));

    let retried = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert_eq!(retried, DeleteLeaseResponse::Deleted { operation_id: None });
    assert!(!build_tag_exists(&core, &tenant).await?);
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);

    let after = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant,
            lease_id,
        })
        .await;
    assert!(matches!(
        after,
        DeleteLeaseResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn reconciler_finishes_an_abandoned_purge() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(PURGE_TAG_DELETE_ATTEMPTS).await?;
    let first = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(matches!(first, DeleteLeaseResponse::Denied(_)));

    // A young marker belongs to its request.
    core.reconcile_pending_purges().await;
    assert!(build_tag_exists(&core, &tenant).await?);

    let mut marker = core.pending_purges.get(&lease_id).await?.context("record")?;
    marker.recorded_at = SystemTime::now() - PENDING_PURGE_RECONCILE_AFTER - Duration::from_secs(1);
    core.pending_purges.put(&marker).await?;
    core.reconcile_pending_purges().await;

    assert!(!build_tag_exists(&core, &tenant).await?);
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);
    Ok(())
}

#[tokio::test]
async fn an_unfinished_purge_keeps_the_tag_of_a_newer_lease_with_the_same_name() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(PURGE_TAG_DELETE_ATTEMPTS).await?;
    let first = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert!(matches!(first, DeleteLeaseResponse::Denied(_)));

    // A newer lease with the same name tagged its snapshot and has since
    // been released, so no live lease holds the name any more.
    core.named_leases.clear(&tenant, "build").await?;
    let newer = Digest::from_str(&format!("sha256:{}", "f".repeat(64)))?;
    let set = core
        .handle_snapshot_tag_set(SetSnapshotTagRequest {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("build")?,
            digest: newer.clone(),
        })
        .await;
    assert_eq!(set, SetSnapshotTagResponse::Applied);

    let retried = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await;
    assert_eq!(retried, DeleteLeaseResponse::Deleted { operation_id: None });
    let kept = core
        .tag_store
        .get(&SnapshotTag {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("build")?,
        })
        .await?
        .ok_or("the newer tag survives the purge")?;
    assert_eq!(kept.digest, newer);
    assert_eq!(core.pending_purges.get(&lease_id).await?, None);
    Ok(())
}

#[tokio::test]
async fn a_purge_never_removes_a_tag_created_after_the_delete_started() -> TestResult {
    let (core, tenant, lease_id) = named_lease_with_flaky_tags(0).await?;
    // No tag under the name when the delete starts.
    let tag = SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new("build")?,
    };
    let (_, revision) = core.tag_store.get_entry(&tag).await?.context("record")?;
    core.tag_store.clear(&tag, revision).await?;

    // A delete that released the lease but stopped before its purge.
    core.pending_purges
        .put(&PendingPurge {
            quota_reservation: None,
            lease_id: lease_id.clone(),
            tenant: tenant.clone(),
            name: Some("build".into()),
            tag_revision: None,
            release_announced: None,
            pending_refund: None,
            drive_claims: None,
            announcement: None,
            recorded_at: SystemTime::now(),
        })
        .await?;
    core.active_leases.delete(&lease_id).await?;

    let digest = Digest::from_str(&format!("sha256:{}", "f".repeat(64)))?;
    let set = core
        .handle_snapshot_tag_set(SetSnapshotTagRequest {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("build")?,
            digest,
        })
        .await;
    assert_eq!(set, SetSnapshotTagResponse::Applied);

    let retried = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id,
        })
        .await;
    assert_eq!(retried, DeleteLeaseResponse::Deleted { operation_id: None });
    assert!(build_tag_exists(&core, &tenant).await?);
    Ok(())
}
