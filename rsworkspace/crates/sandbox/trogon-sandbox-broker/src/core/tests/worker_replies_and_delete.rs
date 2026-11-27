use super::*;

#[tokio::test]
async fn snapshot_tag_delete_not_found_for_missing_tag() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_snapshot_tag_delete(DeleteSnapshotTagRequest {
            tenant: TenantId::new("test-tenant")?,
            name: SnapshotTagName::new("never-set")?,
        })
        .await;

    assert_eq!(response, DeleteSnapshotTagResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_worker_launch_failure_reaches_the_caller_with_its_reason() -> TestResult {
    let core = make_core(Bytes::from_static(
        br#"{"provider_error":"image guest:other is not served by this worker"}"#,
    ))?;

    let response = core.handle_lease_request(make_request(vec![ProviderKind::Mock])?).await;

    assert_eq!(
        response,
        LeaseResponse::Denied(LeaseError::ProviderLaunchFailed {
            provider: ProviderKind::Mock,
            reason: "image guest:other is not served by this worker".to_string(),
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_worker_that_cannot_decode_the_dispatch_is_reported_as_rejecting_it() -> TestResult {
    let core = make_core(Bytes::from_static(br#"{"decode_error":"missing field `lease_id`"}"#))?;

    let response = core.handle_lease_request(make_request(vec![ProviderKind::Mock])?).await;

    assert_eq!(
        response,
        LeaseResponse::Denied(LeaseError::ProviderRejectedDispatch {
            provider: ProviderKind::Mock,
            reason: "missing field `lease_id`".to_string(),
        })
    );
    Ok(())
}

#[tokio::test]
async fn an_unrecognized_worker_reply_is_a_backend_error() -> TestResult {
    let core = make_core(Bytes::from_static(br#"{"unexpected":true}"#))?;

    let response = core.handle_lease_request(make_request(vec![ProviderKind::Mock])?).await;

    assert!(
        matches!(
            response,
            LeaseResponse::Denied(LeaseError::Backend { ref message })
                if message.starts_with("provider reply decode failed")
        ),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn delete_named_sandbox_purges_an_active_named_lease() -> TestResult {
    let core = make_core_with_purge_named_response(PurgeNamedResponse::NotFound)?;
    let tenant = TenantId::new("test-tenant")?;

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant.clone();
    req.name = Some(LeaseName::new("build")?);
    let granted = core.handle_lease_request(req).await;
    assert!(
        matches!(granted, LeaseResponse::Granted(_)),
        "expected Granted, got {granted:?}"
    );

    let response = core
        .handle_delete_named_sandbox(DeleteNamedSandboxRequest {
            tenant: tenant.clone(),
            name: LeaseName::new("build")?,
        })
        .await;
    assert_eq!(response, DeleteNamedSandboxResponse::Deleted);

    // Nothing left to find the second time around.
    let response = core
        .handle_delete_named_sandbox(DeleteNamedSandboxRequest {
            tenant,
            name: LeaseName::new("build")?,
        })
        .await;
    assert_eq!(response, DeleteNamedSandboxResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn delete_lease_by_id_removes_the_named_snapshot_tag() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let tenant = TenantId::new("test-tenant")?;

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant.clone();
    req.name = Some(LeaseName::new("build")?);
    let LeaseResponse::Granted(grant) = core.handle_lease_request(req).await else {
        return Err("expected Granted".into());
    };

    let digest = Digest::from_str(&format!("sha256:{}", "e".repeat(64)))?;
    let set = core
        .handle_snapshot_tag_set(SetSnapshotTagRequest {
            tenant: tenant.clone(),
            name: SnapshotTagName::new("build")?,
            digest,
        })
        .await;
    assert_eq!(set, SetSnapshotTagResponse::Applied);

    let deleted = core
        .handle_delete_lease(DeleteLeaseRequest {
            operation_id: None,
            tenant: tenant.clone(),
            lease_id: grant.lease_id.clone(),
        })
        .await;
    assert_eq!(deleted, DeleteLeaseResponse::Deleted { operation_id: None });

    let tag = core
        .handle_snapshot_tag_delete(DeleteSnapshotTagRequest {
            tenant,
            name: SnapshotTagName::new("build")?,
        })
        .await;
    assert_eq!(tag, DeleteSnapshotTagResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn delete_named_sandbox_not_found_for_unknown_name() -> TestResult {
    let core = make_core_with_purge_named_response(PurgeNamedResponse::NotFound)?;

    let response = core
        .handle_delete_named_sandbox(DeleteNamedSandboxRequest {
            tenant: TenantId::new("test-tenant")?,
            name: LeaseName::new("never-existed")?,
        })
        .await;

    assert_eq!(response, DeleteNamedSandboxResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn delete_named_sandbox_deleted_when_only_the_provider_actor_remains() -> TestResult {
    let core = make_core_with_purge_named_response(PurgeNamedResponse::Purged)?;

    let response = core
        .handle_delete_named_sandbox(DeleteNamedSandboxRequest {
            tenant: TenantId::new("test-tenant")?,
            name: LeaseName::new("stopped-sandbox")?,
        })
        .await;

    assert_eq!(response, DeleteNamedSandboxResponse::Deleted);
    Ok(())
}
