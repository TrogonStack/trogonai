use super::*;
use trogon_sandbox_contract::SnapshotTagName;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn firecracker_cannot_grant_a_rootfs_snapshot_output_policy() -> TestResult {
    let core = firecracker_format_fixture().await?;
    let request = make_request(vec![ProviderKind::Firecracker])?;
    let request = edit_request_spec(request, |parts| {
        parts.snapshot_out = Some(trogon_sandbox_contract::SnapshotPolicy {
            artifact_type: ArtifactType::SandboxRootfsV1,
            target_repository: "snapshots/test-tenant".into(),
            retention: None,
            prewarm: None,
            tag_name: None,
        });
    })?;
    let response = core.handle_lease_request(request).await;
    assert!(
        matches!(response, LeaseResponse::Denied(_)),
        "unsupported output format must fail admission: {response:?}"
    );
    assert!(core.active_leases.all().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn firecracker_named_persistence_uses_its_portable_snapshot_format() -> TestResult {
    let core = firecracker_format_fixture().await?;
    let mut request = make_request(vec![ProviderKind::Firecracker])?;
    request.name = Some(LeaseName::new("saved")?);
    request.persistent = true.into();
    let response = core.handle_lease_request(request).await;
    let LeaseResponse::Granted(grant) = response else {
        return Err(format!("{response:?}").into());
    };
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    let policy = record.spec.snapshot_out().ok_or("snapshot policy")?;
    assert_eq!(policy.artifact_type, ArtifactType::FirecrackerSnapshotV1);
    // Must match the repository of the push target built on release,
    // or the worker rejects the export as a target mismatch.
    assert_eq!(policy.target_repository, snapshot_repository_for_tenant(&record.tenant));
    Ok(())
}

#[tokio::test]
async fn firecracker_mixed_automatic_persistence_requires_an_explicit_snapshot_format() -> TestResult {
    let core = firecracker_format_fixture().await?;
    let mut request = make_request(vec![ProviderKind::Firecracker, ProviderKind::Fly])?;
    request.name = Some(LeaseName::new("saved")?);
    request.persistent = true.into();
    let response = core.handle_lease_request(request).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "mixed providers need an explicit portable snapshot format: {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn named_persistent_lease_skips_auto_persistence_on_a_provider_without_snapshot_formats() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_profile(&registry, "dt-1", ProviderKind::Daytona, vec![]).await?;
    core.worker_registry = registry;
    let mut request = make_request(vec![ProviderKind::Daytona])?;
    request.name = Some(LeaseName::new("saved")?);
    request.persistent = true.into();
    let response = core.handle_lease_request(request).await;
    let LeaseResponse::Granted(grant) = response else {
        return Err(format!("{response:?}").into());
    };
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    assert!(matches!(record.spec.image(), SandboxImage::Registry { .. }));
    assert!(record.spec.snapshot_out().is_none());
    Ok(())
}

#[tokio::test]
async fn named_persistent_lease_routes_past_a_provider_without_snapshot_formats() -> TestResult {
    let mut core = make_core(mock_provider_response()?)?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_profile(&registry, "dt-1", ProviderKind::Daytona, vec![]).await?;
    inject_profile(
        &registry,
        "mock-1",
        ProviderKind::Mock,
        vec![ArtifactType::SandboxRootfsV1],
    )
    .await?;
    core.worker_registry = registry;
    let mut request = make_request(vec![ProviderKind::Daytona, ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("saved")?);
    request.persistent = true.into();
    let response = core.handle_lease_request(request).await;
    let LeaseResponse::Granted(grant) = response else {
        return Err(format!("{response:?}").into());
    };
    assert_eq!(grant.provider, ProviderKind::Mock);
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    assert!(record.spec.snapshot_out().is_some());
    Ok(())
}

#[test]
fn snapshot_tag_images_and_tagged_exports_infer_needs_named_tags() -> TestResult {
    let plain = make_request(vec![ProviderKind::Mock])?;
    assert!(infer_capability_requirements(&plain).is_none());

    let mut tagged = make_request(vec![ProviderKind::Mock])?;
    tagged.spec = with_persistence(
        tagged.spec,
        tagged.tenant.clone(),
        None,
        PersistentSnapshotOptions {
            tag_name: SnapshotTagName::new("saved")?,
            artifact_type: ArtifactType::SandboxRootfsV1,
            target_repository: "snapshots/test-tenant".into(),
            retention: None,
            prewarm: None,
        },
    )?;
    assert!(
        infer_capability_requirements(&tagged)
            .ok_or("requirements")?
            .needs_named_tags
            .is_required()
    );

    let export_only = make_request(vec![ProviderKind::Mock])?;
    let export_only = edit_request_spec(export_only, |parts| {
        parts.snapshot_out = Some(trogon_sandbox_contract::SnapshotPolicy {
            artifact_type: ArtifactType::SandboxRootfsV1,
            target_repository: "snapshots/test-tenant".into(),
            retention: None,
            prewarm: None,
            tag_name: Some("saved".into()),
        });
    })?;
    assert!(
        infer_capability_requirements(&export_only)
            .ok_or("requirements")?
            .needs_named_tags
            .is_required()
    );
    Ok(())
}

#[test]
fn a_spec_with_snapshots_off_cannot_ask_for_a_snapshot_on_release() -> TestResult {
    let request = make_request(vec![ProviderKind::Mock])?;
    let mut parts = request.spec.into_parts();
    parts.retention = snapshots_off();
    parts.snapshot_out = Some(trogon_sandbox_contract::SnapshotPolicy {
        artifact_type: ArtifactType::SandboxRootfsV1,
        target_repository: "snapshots/test-tenant".into(),
        retention: None,
        prewarm: None,
        tag_name: None,
    });
    let error = SandboxSpec::new(parts).err().map(|e| e.to_string());
    assert!(error.is_some_and(|e| e.contains("retention.snapshots")));
    Ok(())
}

#[tokio::test]
async fn a_named_lease_with_snapshots_off_is_not_auto_captured() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("saved")?);
    request.persistent = true.into();
    let request = edit_request_spec(request, |parts| {
        parts.retention = snapshots_off();
    })?;
    let response = core.handle_lease_request(request).await;
    let LeaseResponse::Granted(grant) = response else {
        return Err(format!("{response:?}").into());
    };
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    assert!(record.spec.snapshot_out().is_none());
    assert_eq!(record.name.as_ref().map(LeaseName::as_str), Some("saved"));

    // The same request with snapshots on is captured under its name.
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("captured")?);
    request.idempotency_key = LeaseRequestKey::new("test-key-2")?;
    let LeaseResponse::Granted(grant) = core.handle_lease_request(request).await else {
        return Err("expected a grant".into());
    };
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    assert!(record.spec.snapshot_out().is_some());
    Ok(())
}
