use super::*;

#[tokio::test]
async fn lease_counters_grow_with_commands_and_file_transfers() -> TestResult {
    let usage: Arc<dyn LeaseUsageStore> = Arc::new(InMemoryLeaseUsageStore::new());
    let core = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        usage,
        Arc::new(InMemoryActiveLeaseStore::new()),
    )?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "usage-grow").await?;

    run_one(&core, &lease_id).await;
    run_one(&core, &lease_id).await;
    let written = core
        .handle_write_files(WriteFilesRequest {
            lease_id: lease_id.clone(),
            files: vec![trogon_sandbox_contract::SandboxFileWrite {
                path: "a".into(),
                content: vec![1; 11],
                mode: None,
            }],
            as_user: None,
        })
        .await;
    assert_eq!(written, WriteFilesResponse::Written);
    let read = core
        .handle_read_file(ReadFileRequest {
            lease_id: lease_id.clone(),
            path: "a".into(),
            as_user: None,
        })
        .await;
    assert!(matches!(read, ReadFileResponse::Content { .. }));

    let u = usage_of(
        core.handle_get_lease_usage(GetLeaseUsageRequest { tenant, lease_id })
            .await,
    )?;
    assert_eq!(u.commands_run, 2);
    assert_eq!(u.command_output_bytes, 10);
    assert_eq!(u.file_bytes_in, 11);
    assert_eq!(u.file_bytes_out, 7);
    assert_eq!(u.status, LeaseStatus::Running);
    assert!(u.ended_at.is_none());
    assert!(u.gauges.is_none(), "unsupported gauges stay absent");
    Ok(())
}

#[tokio::test]
async fn a_second_broker_sharing_the_stores_reports_the_same_figures() -> TestResult {
    let usage: Arc<dyn LeaseUsageStore> = Arc::new(InMemoryLeaseUsageStore::new());
    let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let first = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        Arc::clone(&usage),
        Arc::clone(&active),
    )?;
    let second = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        usage,
        active,
    )?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&first, &tenant, "usage-shared").await?;
    run_one(&first, &lease_id).await;
    run_one(&second, &lease_id).await;

    let req = || GetLeaseUsageRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
    };
    let from_first = usage_of(first.handle_get_lease_usage(req()).await)?;
    let from_second = usage_of(second.handle_get_lease_usage(req()).await)?;
    assert_eq!(from_first.commands_run, 2);
    assert_eq!(from_second.commands_run, 2);
    assert_eq!(from_first.command_output_bytes, from_second.command_output_bytes);
    Ok(())
}

#[tokio::test]
async fn a_released_lease_returns_final_figures_and_stops_counting() -> TestResult {
    let core = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        Arc::new(InMemoryLeaseUsageStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
    )?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "usage-released").await?;
    run_one(&core, &lease_id).await;
    core.release_lease(lease_id.clone(), CancelIntent::Release, LeaseReleasedReason::Cancelled)
        .await?;

    let u = usage_of(
        core.handle_get_lease_usage(GetLeaseUsageRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await,
    )?;
    assert_eq!(u.status, LeaseStatus::Released);
    assert_eq!(u.commands_run, 1);
    assert!(u.ended_at.is_some());

    // Another tenant and an unknown id read as not found.
    for (t, id) in [
        (TenantId::new("intruder")?, lease_id),
        (tenant, LeaseId::new("never-existed")),
    ] {
        let denied = core
            .handle_get_lease_usage(GetLeaseUsageRequest {
                tenant: t,
                lease_id: id,
            })
            .await;
        assert!(matches!(
            denied,
            GetLeaseUsageResponse::Denied(LeaseError::LeaseNotFound { .. })
        ));
    }
    Ok(())
}

#[tokio::test]
async fn gauges_come_from_the_worker_only_when_the_profile_declares_them() -> TestResult {
    for (reports, expect_gauges) in [(true, true), (false, false)] {
        let core = usage_core(
            gauge_registry(reports).await?,
            some_gauges(),
            Arc::new(InMemoryLeaseUsageStore::new()),
            Arc::new(InMemoryActiveLeaseStore::new()),
        )?;
        let tenant = TenantId::new("acme")?;
        let lease_id = running_lease_in(&core, &tenant, "usage-gauges").await?;
        let u = usage_of(
            core.handle_get_lease_usage(GetLeaseUsageRequest { tenant, lease_id })
                .await,
        )?;
        assert_eq!(u.gauges.is_some(), expect_gauges, "reports={reports}");
        if let Some(g) = u.gauges {
            assert_eq!(g.cpu_nanos_total, Some(42));
            assert_eq!(g.memory_peak_bytes, None);
        }
    }
    Ok(())
}

#[tokio::test]
async fn paused_time_is_reported_apart_from_running_time() -> TestResult {
    let core = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        Arc::new(InMemoryLeaseUsageStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
    )?;
    let tenant = TenantId::new("acme")?;
    let lease_id = running_lease_in(&core, &tenant, "usage-paused").await?;
    core.handle_pause_lease(PauseLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        max_paused_seconds: None,
        operation_id: None,
    })
    .await;
    let u = usage_of(
        core.handle_get_lease_usage(GetLeaseUsageRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
        })
        .await,
    )?;
    assert_eq!(u.status, LeaseStatus::Paused);
    assert!(u.gauges.is_none(), "a paused sandbox is not sampled");
    core.handle_resume_lease(ResumeLeaseRequest {
        tenant: tenant.clone(),
        lease_id: lease_id.clone(),
        operation_id: None,
    })
    .await;
    let u = usage_of(
        core.handle_get_lease_usage(GetLeaseUsageRequest { tenant, lease_id })
            .await,
    )?;
    assert_eq!(u.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn tenant_usage_sums_that_tenants_live_and_released_leases() -> TestResult {
    let usage_store: Arc<dyn LeaseUsageStore> = Arc::new(InMemoryLeaseUsageStore::new());
    let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let core = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        Arc::clone(&usage_store),
        Arc::clone(&active),
    )?;
    let acme = TenantId::new("acme")?;
    let other = TenantId::new("other")?;
    let a = running_lease_in(&core, &acme, "usage-t-a").await?;
    let b = running_lease_in(&core, &acme, "usage-t-b").await?;
    let gone = running_lease_in(&core, &acme, "usage-t-gone").await?;
    let c = running_lease_in(&core, &other, "usage-t-c").await?;
    for id in [&a, &b, &gone, &c] {
        run_one(&core, id).await;
    }
    let released = active.get(&gone).await?.ok_or("active record missing")?;
    usage_store.finish(&released, SystemTime::now()).await?;
    active.delete(&gone).await?;

    let GetTenantUsageResponse::Usage { usage } = core
        .handle_get_tenant_usage(GetTenantUsageRequest { tenant: acme })
        .await
    else {
        return Err("tenant usage should succeed".into());
    };
    assert_eq!(usage.live_leases, 2);
    assert_eq!(usage.ended_leases, 1);
    assert_eq!(usage.commands_run, 3);
    assert_eq!(usage.command_output_bytes, 15);
    Ok(())
}

#[tokio::test]
async fn tenant_usage_skips_an_entry_whose_lease_is_mid_release() -> TestResult {
    let usage_store: Arc<dyn LeaseUsageStore> = Arc::new(InMemoryLeaseUsageStore::new());
    let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
    let core = usage_core(
        gauge_registry(false).await?,
        ResourceUsageResponse::Unsupported,
        Arc::clone(&usage_store),
        Arc::clone(&active),
    )?;
    let acme = TenantId::new("acme")?;
    let lease = running_lease_in(&core, &acme, "usage-t-mid").await?;
    run_one(&core, &lease).await;
    // The record is gone but the usage entry was not finished yet.
    active.delete(&lease).await?;

    let GetTenantUsageResponse::Usage { usage } = core
        .handle_get_tenant_usage(GetTenantUsageRequest { tenant: acme })
        .await
    else {
        return Err("tenant usage should succeed".into());
    };
    assert_eq!(usage, TenantUsage::default());
    Ok(())
}
