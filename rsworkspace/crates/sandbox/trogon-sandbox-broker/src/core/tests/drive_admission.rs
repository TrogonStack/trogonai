use super::*;
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_contract::{ProviderDispatchPayload, RemoteMountSource};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn delete_drive_denied_when_attached() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-holding-drive");
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    drive_store
        .set_attached_lease(&drive_id, Some(lease_id.clone()))
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store)?;

    let response = core
        .handle_delete_drive(DeleteDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
        })
        .await;

    assert!(
        matches!(response, DeleteDriveResponse::Denied(DriveError::DriveAttached { .. })),
        "expected DriveAttached denial, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn delete_drive_applied_when_not_attached() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?;

    let response = core
        .handle_delete_drive(DeleteDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
        })
        .await;

    assert!(
        matches!(response, DeleteDriveResponse::Applied),
        "expected Applied, got {response:?}"
    );
    assert!(drive_store.get(&drive_id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn delete_drive_purges_storage_before_removing_record() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let purger = Arc::new(RecordingDriveDataPurger::new());
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?
        .with_drive_data_purger(purger.clone());

    let response = core
        .handle_delete_drive(DeleteDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
        })
        .await;

    assert!(
        matches!(response, DeleteDriveResponse::Applied),
        "expected Applied, got {response:?}"
    );
    let purged = purger.purged_records()?;
    assert!(
        purged.iter().any(|r| r.id == drive_id),
        "expected purger to have been called with the deleted drive's record, got {purged:?}"
    );
    assert!(drive_store.get(&drive_id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn delete_drive_denied_when_purge_fails() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?
        .with_drive_data_purger(Arc::new(FailingDriveDataPurger));

    let response = core
        .handle_delete_drive(DeleteDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
        })
        .await;

    assert!(
        matches!(response, DeleteDriveResponse::Denied(DriveError::StorageBackend { .. })),
        "expected StorageBackend denial, got {response:?}"
    );
    assert!(
        drive_store.get(&drive_id).await?.is_some(),
        "drive record must not be removed when storage purge fails"
    );
    Ok(())
}

#[tokio::test]
async fn lease_admission_denied_when_drive_not_found() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let core = make_core(mock_provider_response()?)?;
    let missing_drive = make_drive_id(&tenant, "missing")?;

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant.clone();
    let req = {
        let mut req = req;
        let mut parts = req.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: missing_drive,
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        }];
        req.spec = SandboxSpec::new(parts)?;
        req
    };

    let response = core.handle_lease_request(req).await;

    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected denial for missing drive, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn handle_lease_request_with_secret_looking_env_completes_cleanly() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let req = edit_request_spec(req, |parts| {
        parts
            .env
            .insert("API_SECRET".to_string(), "super-secret-value".to_string());
    })?;

    let _response = core.handle_lease_request(req).await;
    Ok(())
}

#[test]
fn a_spec_with_too_many_drives_cannot_be_built() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let mut drives = Vec::new();
    for i in 0..(MAX_DRIVES_PER_SANDBOX + 1) {
        drives.push(DriveMount {
            drive_id: make_drive_id(&tenant, &format!("drive-{i}"))?,
            mount_path: MountPath::new(format!("/mnt/drive-{i}"))?,
            mode: DriveMountMode::default(),
        });
    }
    let mut parts = spec_parts()?;
    parts.drives = drives;

    let error = SandboxSpec::new(parts).err().ok_or("too many drives accepted")?;

    assert!(
        matches!(
            error,
            trogon_sandbox_contract::SandboxSpecError::Drives(
                trogon_sandbox_contract::DriveError::TooManyDrives { .. }
            )
        ),
        "expected a drives error, got {error:?}"
    );
    Ok(())
}

#[test]
fn a_spec_with_duplicate_exposed_ports_cannot_be_built() -> TestResult {
    let mut parts = spec_parts()?;
    parts.exposed_ports = vec![
        trogon_sandbox_contract::Port::new(3000)?,
        trogon_sandbox_contract::Port::new(3000)?,
    ];

    let error = SandboxSpec::new(parts).err().ok_or("duplicate ports accepted")?;

    assert!(
        matches!(
            error,
            trogon_sandbox_contract::SandboxSpecError::DuplicateExposedPort(_)
        ),
        "expected duplicate exposed port, got {error:?}"
    );
    Ok(())
}

#[test]
fn an_fs_policy_with_an_inert_deny_glob_cannot_be_built() -> TestResult {
    let pattern = trogon_sandbox_contract::GlobPattern::new("/data/*/")?;

    let error = trogon_sandbox_contract::FsPolicy::new(
        vec![trogon_sandbox_contract::FsPathEntry::Glob { pattern }],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .err()
    .ok_or("inert deny glob accepted")?;

    assert!(
        matches!(error, trogon_sandbox_contract::FsPolicyError::InertDenyGlob { .. }),
        "expected inert deny glob, got {error:?}"
    );
    Ok(())
}

#[test]
fn a_spec_with_overlapping_mount_paths_cannot_be_built() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let mut parts = make_request(vec![ProviderKind::Mock])?.spec.into_parts();
    parts.drives = vec![
        DriveMount {
            drive_id: make_drive_id(&tenant, "drive-a")?,
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        },
        DriveMount {
            drive_id: make_drive_id(&tenant, "drive-b")?,
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        },
    ];
    assert!(matches!(
        SandboxSpec::new(parts),
        Err(trogon_sandbox_contract::SandboxSpecError::Drives(
            DriveError::DriveMountPathOverlap { .. }
        ))
    ));
    Ok(())
}

#[tokio::test]
async fn lease_grant_marks_referenced_drives_attached() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?;

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant.clone();
    let req = {
        let mut req = req;
        let mut parts = req.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive_id.clone(),
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        }];
        req.spec = SandboxSpec::new(parts)?;
        req
    };

    let response = core.handle_lease_request(req).await;
    let granted = match response {
        LeaseResponse::Granted(granted) => granted,
        LeaseResponse::Denied(e) => return Err(format!("expected grant, got denial: {e}").into()),
    };

    let record = drive_store.get(&drive_id).await?.ok_or("missing record")?;
    assert_eq!(record.attached_lease, Some(granted.lease_id));
    Ok(())
}

#[tokio::test]
async fn lease_admission_denied_when_drive_already_attached_to_another_lease() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?;

    let mut first_req = make_request(vec![ProviderKind::Mock])?;
    first_req.tenant = tenant.clone();
    first_req.idempotency_key = LeaseRequestKey::new("first-lease-key")?;
    let first_req = {
        let mut first_req = first_req;
        let mut parts = first_req.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive_id.clone(),
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        }];
        first_req.spec = SandboxSpec::new(parts)?;
        first_req
    };

    let first_response = core.handle_lease_request(first_req).await;
    let first_granted = match first_response {
        LeaseResponse::Granted(granted) => granted,
        LeaseResponse::Denied(e) => return Err(format!("expected first lease to be granted, got denial: {e}").into()),
    };

    let mut second_req = make_request(vec![ProviderKind::Mock])?;
    second_req.tenant = tenant.clone();
    second_req.idempotency_key = LeaseRequestKey::new("second-lease-key")?;
    let second_req = {
        let mut second_req = second_req;
        let mut parts = second_req.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive_id.clone(),
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        }];
        second_req.spec = SandboxSpec::new(parts)?;
        second_req
    };

    let second_response = core.handle_lease_request(second_req).await;
    assert!(
        matches!(second_response, LeaseResponse::Denied(_)),
        "expected second lease request to be denied, got {second_response:?}"
    );

    let record = drive_store.get(&drive_id).await?.ok_or("missing record")?;
    assert_eq!(
        record.attached_lease,
        Some(first_granted.lease_id),
        "failed second attempt must not clobber the first lease's attachment"
    );
    Ok(())
}

#[tokio::test]
async fn lease_release_clears_attached_drives() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?;

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant.clone();
    let req = {
        let mut req = req;
        let mut parts = req.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive_id.clone(),
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::default(),
        }];
        req.spec = SandboxSpec::new(parts)?;
        req
    };

    let response = core.handle_lease_request(req).await;
    let granted = match response {
        LeaseResponse::Granted(granted) => granted,
        LeaseResponse::Denied(e) => return Err(format!("expected grant, got denial: {e}").into()),
    };

    core.handle_cancel(granted.lease_id, LeaseReleasedReason::Cancelled)
        .await?;

    let record = drive_store.get(&drive_id).await?.ok_or("missing record")?;
    assert_eq!(record.attached_lease, None);
    Ok(())
}

#[tokio::test]
async fn a_cancel_releases_the_lease_with_the_reason_it_was_sent_with() -> TestResult {
    for reason in [LeaseReleasedReason::Cancelled, LeaseReleasedReason::Failed] {
        let bus = Arc::new(ReleaseRecordingBus {
            released: std::sync::Mutex::new(Vec::new()),
        });
        let core = make_core_with_bus(bus.clone())?;
        let granted = match core.handle_lease_request(make_request(vec![ProviderKind::Mock])?).await {
            LeaseResponse::Granted(granted) => granted,
            LeaseResponse::Denied(e) => return Err(format!("expected grant, got denial: {e}").into()),
        };

        core.handle_cancel(granted.lease_id.clone(), reason).await?;

        let released = locked(&bus.released)?;
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].lease_id, granted.lease_id);
        assert_eq!(released[0].reason, reason);
    }
    Ok(())
}

#[tokio::test]
async fn granted_lease_dispatch_payload_includes_remote_mount_derived_from_drive() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;

    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let core = BrokerCore::new(
        bus.clone(),
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
        default_pins(),
        default_tag_store(),
        default_resolved_digests(),
        drive_store,
        default_named_leases(),
        default_config(),
        BrokerMetrics::new()?,
    );

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant.clone();
    let req = {
        let mut req = req;
        let mut parts = req.spec.into_parts();
        parts.drives = vec![DriveMount {
            drive_id: drive_id.clone(),
            mount_path: MountPath::new("/mnt/data")?,
            mode: DriveMountMode::ReadOnly,
        }];
        req.spec = SandboxSpec::new(parts)?;
        req
    };

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Granted(_)),
        "expected grant, got {response:?}"
    );

    let dispatched_payload = locked(&bus.last_payload)?.clone().ok_or("dispatched payload")?;
    let envelope: ProviderDispatchPayload = serde_json::from_slice(&dispatched_payload)?;

    let mount = envelope
        .request
        .spec
        .mounts()
        .iter()
        .find(|m| m.mount_path.as_str() == "/mnt/data")
        .ok_or("mount")?;
    assert!(mount.access.is_read_only());
    assert_eq!(
        mount.source,
        RemoteMountSource::S3 {
            bucket: trogon_sandbox_contract::S3BucketName::new("test-bucket")?,
            prefix: format!("drives/{}/data/", tenant.as_str()),
        }
    );
    Ok(())
}

#[tokio::test]
async fn network_policy_update_dispatches_to_owning_worker_with_expected_subject() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let bus = Arc::new(CapturingBus::new(network_policy_applied_response()?));
    let core = make_core_with_bus(bus.clone())?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, NetworkPolicy::DenyAll))
        .await;

    assert!(
        matches!(response, NetworkPolicyUpdateResponse::Applied),
        "expected Applied, got {response:?}"
    );

    let sent = locked(&bus.last_payload)?.clone().ok_or("dispatched payload")?;
    let sent: NetworkPolicyUpdateRequest = serde_json::from_slice(&sent)?;
    assert_eq!(sent.lease_id, lease_id);
    assert_eq!(sent.network_policy, NetworkPolicy::DenyAll);
    Ok(())
}

#[tokio::test]
async fn network_policy_update_propagates_worker_denial() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(network_policy_worker_denied_response()?)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, NetworkPolicy::DenyAll))
        .await;

    assert!(
        matches!(
            response,
            NetworkPolicyUpdateResponse::Denied(LeaseError::Backend { .. })
        ),
        "expected worker denial to propagate, got {response:?}"
    );

    // The record's `network_policy` is already persisted before dispatch,
    // even though the worker denied applying it live -- matching
    // `handle_network_policy_update`'s doc comment: this method updates
    // the broker's record of intent, and reports whether the running
    // sandbox's live proxy actually picked it up.
    let updated = core.active_leases.get(&lease_id).await?.ok_or("missing record")?;
    assert_eq!(updated.spec.network_policy(), Some(&NetworkPolicy::DenyAll));
    Ok(())
}

#[tokio::test]
async fn valid_request_returns_granted() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}
