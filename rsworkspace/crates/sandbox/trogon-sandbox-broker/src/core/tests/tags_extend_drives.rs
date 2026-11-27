use super::*;
use crate::constants::{MAX_DRIVE_SIZE_GIB, MAX_LEASE_TTL_SECONDS};
use trogon_sandbox_contract::DriveName;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn update_lease_tags_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_update_lease_tags(make_update_lease_tags_request(
            &LeaseId::new("missing"),
            SandboxTags::default(),
        ))
        .await;

    assert!(matches!(
        response,
        UpdateLeaseTagsResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn update_lease_tags_denied_when_lease_not_running() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.status = LeaseStatus::Released;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_update_lease_tags(make_update_lease_tags_request(&lease_id, SandboxTags::default()))
        .await;

    assert!(matches!(
        response,
        UpdateLeaseTagsResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn update_lease_tags_applies_and_replaces_not_merges_existing_tags() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.tags = SandboxTags::new(BTreeMap::from([("env".to_string(), "staging".to_string())]))?;
    core.active_leases.put(&record).await?;

    let new_tags = SandboxTags::new(BTreeMap::from([("team".to_string(), "platform".to_string())]))?;
    let response = core
        .handle_update_lease_tags(make_update_lease_tags_request(&lease_id, new_tags.clone()))
        .await;

    assert!(
        matches!(response, UpdateLeaseTagsResponse::Applied),
        "expected Applied, got {response:?}"
    );

    let updated = core.active_leases.get(&lease_id).await?.ok_or("lease record")?;
    assert_eq!(updated.tags, new_tags);
    assert_eq!(updated.tags.get("env"), None);
    assert_eq!(updated.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn extend_lease_timeout_denied_when_it_would_exceed_max_lifetime() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, MAX_LEASE_TTL_SECONDS + 1))
        .await;

    assert!(matches!(
        response,
        ExtendLeaseTimeoutResponse::Denied(LeaseError::WouldExceedMaxLifetime {
            max
        }) if max == MAX_LEASE_TTL_SECONDS
    ));

    let unchanged = core.active_leases.get(&lease_id).await?.ok_or("lease record")?;
    assert_eq!(unchanged.deadline, record.deadline);
    Ok(())
}

#[tokio::test]
async fn extend_lease_timeout_many_small_extensions_cannot_bypass_ceiling() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let small_step = 3600u32; // 1 hour per extension
    loop {
        let response = core
            .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, small_step))
            .await;
        match response {
            ExtendLeaseTimeoutResponse::Extended { new_deadline } => {
                let total = new_deadline.duration_since(record.created_at)?;
                assert!(
                    total <= Duration::from_secs(u64::from(MAX_LEASE_TTL_SECONDS)),
                    "accepted extension pushed total lifetime past the ceiling"
                );
            }
            ExtendLeaseTimeoutResponse::Denied(LeaseError::WouldExceedMaxLifetime { max }) => {
                assert_eq!(max, MAX_LEASE_TTL_SECONDS);
                break;
            }
            other => return Err(format!("unexpected response: {other:?}").into()),
        }
    }

    // Once denied, further small-step attempts must still be denied; this
    // confirms the ceiling isn't a one-shot fluke and the stored deadline
    // was never advanced past it.
    let response = core
        .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, small_step))
        .await;
    assert!(matches!(
        response,
        ExtendLeaseTimeoutResponse::Denied(LeaseError::WouldExceedMaxLifetime {
            max
        }) if max == MAX_LEASE_TTL_SECONDS
    ));
    Ok(())
}

#[tokio::test]
async fn extend_lease_timeout_succeeds_and_persists_new_deadline() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_extend_lease_timeout(make_extend_lease_timeout_request(&lease_id, 60))
        .await;

    let expected_deadline = record.deadline + Duration::from_secs(60);
    assert!(
        matches!(response, ExtendLeaseTimeoutResponse::Extended { new_deadline } if new_deadline == expected_deadline),
        "expected Extended with advanced deadline, got {response:?}"
    );

    let updated = core.active_leases.get(&lease_id).await?.ok_or("lease record")?;
    assert_eq!(updated.deadline, expected_deadline);
    assert_eq!(updated.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn create_drive_returns_already_exists_with_record_on_second_create() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store.clone())?;
    let drive_id = make_drive_id(&tenant, "data")?;

    let first = core
        .handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
            size_gib: None,
            storage: trogon_sandbox_contract::DriveStorage::S3 {
                bucket: "test-bucket".to_string(),
            },
        })
        .await;
    assert!(
        matches!(first, CreateDriveResponse::Created(_)),
        "expected Created on first create, got {first:?}"
    );

    let second = core
        .handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
            size_gib: None,
            storage: trogon_sandbox_contract::DriveStorage::S3 {
                bucket: "test-bucket".to_string(),
            },
        })
        .await;

    match second {
        CreateDriveResponse::AlreadyExists(record) => {
            assert_eq!(record.id, drive_id);
            assert_eq!(record.tenant, tenant);
            assert_eq!(record.storage.s3_bucket(), Some("test-bucket"));
        }
        other => return Err(format!("expected AlreadyExists with record, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn create_drive_denied_when_size_exceeds_max() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let core = make_core(mock_provider_response()?)?;
    let drive_id = make_drive_id(&tenant, "data")?;

    let response = core
        .handle_create_drive(CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: drive_id.clone(),
            size_gib: Some(MAX_DRIVE_SIZE_GIB + 1),
            storage: trogon_sandbox_contract::DriveStorage::S3 {
                bucket: "test-bucket".to_string(),
            },
        })
        .await;

    assert!(
        matches!(
            response,
            CreateDriveResponse::Denied(DriveError::DriveSizeExceeded { .. })
        ),
        "expected DriveSizeExceeded denial, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn list_drives_returns_only_tenant_drives() -> TestResult {
    let tenant_a = TenantId::new("tenant-a")?;
    let tenant_b = TenantId::new("tenant-b")?;
    let drive_store = default_drive_store();
    drive_store
        .create(
            &tenant_a,
            &make_drive_id(&tenant_a, "alpha")?,
            10,
            "test-bucket".to_string(),
        )
        .await?;
    drive_store
        .create(
            &tenant_b,
            &make_drive_id(&tenant_b, "beta")?,
            10,
            "test-bucket".to_string(),
        )
        .await?;
    let core = make_core_with_drive_store(mock_provider_response()?, drive_store)?;

    let response = core
        .handle_list_drives(ListDrivesRequest {
            tenant: tenant_a.clone(),
        })
        .await;

    assert_eq!(response.drives.len(), 1);
    assert_eq!(response.drives[0].id.tenant, tenant_a);
    assert_eq!(response.drives[0].id.name, DriveName::new("alpha")?);
    Ok(())
}
