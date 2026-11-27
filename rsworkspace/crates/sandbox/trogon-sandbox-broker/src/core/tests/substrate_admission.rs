use super::*;

fn substrate_request() -> anyhow::Result<LeaseRequest> {
    make_request(vec![ProviderKind::Substrate])
}

#[test]
fn build_dispatch_payload_includes_correlation_id() -> TestResult {
    let correlation_id = CorrelationId::new("corr-123");
    let req = make_request(vec![ProviderKind::Mock])?;
    let lease_id = LeaseId::new("lease-1");
    let bytes = build_dispatch_payload(&req, &lease_id, &correlation_id, &[])?;
    let payload: trogon_sandbox_contract::ProviderDispatchPayload = serde_json::from_slice(&bytes)?;
    assert_eq!(payload.correlation_id.as_ref().map(|c| c.as_str()), Some("corr-123"));
    Ok(())
}

#[test]
fn substrate_accepts_a_plain_spec() -> TestResult {
    let req = substrate_request()?;
    assert_eq!(super::substrate_unsupported_spec_reason(&req.spec), None);
    Ok(())
}

#[test]
fn substrate_accepts_a_single_exposed_port() -> TestResult {
    let port = trogon_sandbox_contract::Port::new(8080)?;
    let req = edit_request_spec(substrate_request()?, |parts| {
        parts.exposed_ports = vec![port];
    })?;
    assert_eq!(req.spec.exposed_ports().len(), 1);
    assert_eq!(super::substrate_unsupported_spec_reason(&req.spec), None);
    Ok(())
}

#[test]
fn substrate_rejects_privileged() -> TestResult {
    let req = edit_request_spec(substrate_request()?, |parts| {
        parts.privileged = trogon_sandbox_contract::PrivilegedAccess::Granted;
    })?;
    assert!(super::substrate_unsupported_spec_reason(&req.spec).is_some_and(|r| r.contains("privileged")));
    Ok(())
}

#[test]
fn substrate_rejects_user_override() -> TestResult {
    let req = edit_request_spec(substrate_request()?, |parts| {
        parts.user = Some("1000:1000".to_string());
    })?;
    assert!(super::substrate_unsupported_spec_reason(&req.spec).is_some_and(|r| r.contains("user overrides")));
    Ok(())
}

#[test]
fn substrate_rejects_memory_above_the_largest_size_class() -> TestResult {
    let above = NonZeroU32::new(crate::constants::SUBSTRATE_MAX_MEMORY_MIB + 1).context("zero memory")?;
    let req = edit_request_spec(substrate_request()?, |parts| {
        parts.memory_mib = above;
    })?;
    assert!(super::substrate_unsupported_spec_reason(&req.spec).is_some_and(|r| r.contains("size class")));
    Ok(())
}

#[tokio::test]
async fn substrate_rejects_drives_before_dispatch() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let drive = DriveMount {
        drive_id: make_drive_id(&TenantId::new("test-tenant")?, "scratch")?,
        mount_path: MountPath::new("/mnt/scratch")?,
        mode: DriveMountMode::default(),
    };
    let req = edit_request_spec(substrate_request()?, |parts| {
        parts.drives = vec![drive];
    })?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected InvalidSpec for substrate + drives, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn substrate_rejects_host_mount_workspace() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_host_mount_request(vec![ProviderKind::Substrate], "/srv/workspaces/acme")?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "expected InvalidSpec for substrate + host_mount, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn fork_grants_new_lease_and_leaves_source_running() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let source_lease_id = LeaseId::new("lease-source");
    let core = make_core_with_bus(Arc::new(RoutingBus {
        checkpoint_response: checkpoint_ok_response()?,
        dispatch_response: mock_provider_response()?,
    }))?;
    let record = running_source_record(&tenant, &source_lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_fork_request(make_fork_request(&tenant, &source_lease_id))
        .await;

    match response {
        LeaseResponse::Granted(granted) => {
            assert_ne!(granted.lease_id, source_lease_id);
        }
        LeaseResponse::Denied(e) => return Err(format!("expected grant, got denial: {e}").into()),
    }

    let source_still_running = core
        .active_leases
        .get(&source_lease_id)
        .await?
        .ok_or("source lease record should still exist")?;
    assert_eq!(source_still_running.status, LeaseStatus::Running);
    Ok(())
}
