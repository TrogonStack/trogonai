use super::*;

#[tokio::test]
async fn request_below_tenant_specific_cap_is_granted() -> TestResult {
    let limits = Arc::new(InMemoryTenantLimits::new());
    let tenant = TenantId::new("test-tenant")?;
    limits
        .insert(
            &tenant,
            TenantLimit {
                max_cpu_seconds_per_request: 120,
                max_memory_mib_per_request: 1024,
                per_tag_quota: None,
            },
        )
        .await;
    let core = make_core_with_limits(mock_provider_response()?, limits, 3600, 8192).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn account_limits_report_caps_and_per_tag_consumption() -> TestResult {
    let (core, tenant) = per_tag_core(2).await?;
    for (user, key) in [("alice", "a1"), ("alice", "a2"), ("bob", "b1")] {
        assert!(matches!(
            core.handle_lease_request(user_request(Some(user), key)?).await,
            LeaseResponse::Granted(_)
        ));
    }
    let GetAccountLimitsResponse::Limits { limits } = core
        .handle_get_account_limits(GetAccountLimitsRequest { tenant: tenant.clone() })
        .await
    else {
        return Err("expected limits".into());
    };
    assert_eq!(limits.active_leases, 3);
    assert_eq!(limits.max_cpu_seconds_per_request, 3600);
    assert_eq!(limits.max_memory_mib_per_request, 8192);
    let tag = limits.per_tag.ok_or("per-tag policy")?;
    assert_eq!(tag.tag_key, "user");
    assert_eq!(tag.max_active_leases, 2);
    assert_eq!(tag.in_use.get("alice"), Some(&2));
    assert_eq!(tag.in_use.get("bob"), Some(&1));

    // Another tenant sees none of it.
    let GetAccountLimitsResponse::Limits { limits } = core
        .handle_get_account_limits(GetAccountLimitsRequest {
            tenant: TenantId::new("someone-else")?,
        })
        .await
    else {
        return Err("expected limits".into());
    };
    assert_eq!(limits.active_leases, 0);
    assert!(limits.per_tag.is_none());
    Ok(())
}

#[tokio::test]
async fn per_tag_quota_denies_a_user_at_the_cap_but_not_other_users() -> TestResult {
    let (core, _) = per_tag_core(2).await?;
    for key in ["a1", "a2"] {
        assert!(matches!(
            core.handle_lease_request(user_request(Some("alice"), key)?).await,
            LeaseResponse::Granted(_)
        ));
    }
    let denied = core.handle_lease_request(user_request(Some("alice"), "a3")?).await;
    match denied {
        LeaseResponse::Denied(LeaseError::TooManyTaggedLeases {
            tag_key,
            tag_value,
            cap,
            ..
        }) => {
            assert_eq!(tag_key, "user");
            assert_eq!(tag_value, "alice");
            assert_eq!(cap, 2);
        }
        other => return Err(format!("expected TooManyTaggedLeases, got {other:?}").into()),
    }
    assert!(matches!(
        core.handle_lease_request(user_request(Some("bob"), "b1")?).await,
        LeaseResponse::Granted(_)
    ));
    assert!(matches!(
        core.handle_lease_request(user_request(None, "untagged")?).await,
        LeaseResponse::Granted(_)
    ));
    Ok(())
}

#[tokio::test]
async fn per_tag_quota_frees_a_slot_when_a_lease_ends() -> TestResult {
    let (core, tenant) = per_tag_core(1).await?;
    let LeaseResponse::Granted(grant) = core.handle_lease_request(user_request(Some("alice"), "a1")?).await else {
        return Err("expected a grant".into());
    };
    assert!(matches!(
        core.handle_lease_request(user_request(Some("alice"), "a2")?).await,
        LeaseResponse::Denied(LeaseError::TooManyTaggedLeases { .. })
    ));
    let mut record = core.active_leases.get(&grant.lease_id).await?.ok_or("lease record")?;
    assert_eq!(record.tenant, tenant);
    record.status = LeaseStatus::Released;
    core.active_leases.put(&record).await?;
    assert!(matches!(
        core.handle_lease_request(user_request(Some("alice"), "a3")?).await,
        LeaseResponse::Granted(_)
    ));
    Ok(())
}

#[tokio::test]
async fn per_tag_quota_replay_of_a_granted_request_is_not_denied() -> TestResult {
    let (core, _) = per_tag_core(1).await?;
    let first = core.handle_lease_request(user_request(Some("alice"), "same")?).await;
    let replay = core.handle_lease_request(user_request(Some("alice"), "same")?).await;
    assert!(matches!(first, LeaseResponse::Granted(_)));
    assert!(matches!(replay, LeaseResponse::Granted(_)));
    Ok(())
}

#[test]
fn tenant_limit_without_per_tag_quota_still_parses() -> TestResult {
    let limit: TenantLimit =
        serde_json::from_str(r#"{"max_cpu_seconds_per_request":10,"max_memory_mib_per_request":64}"#)?;
    assert!(limit.per_tag_quota.is_none());
    Ok(())
}

#[tokio::test]
async fn request_above_tenant_specific_cap_is_denied() -> TestResult {
    let limits = Arc::new(InMemoryTenantLimits::new());
    let tenant = TenantId::new("test-tenant")?;
    limits
        .insert(
            &tenant,
            TenantLimit {
                max_cpu_seconds_per_request: 30,
                max_memory_mib_per_request: 1024,
                per_tag_quota: None,
            },
        )
        .await;
    let core = make_core_with_limits(mock_provider_response()?, limits, 3600, 8192).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn request_above_broker_default_with_no_tenant_override_is_denied() -> TestResult {
    let limits = Arc::new(InMemoryTenantLimits::new());
    let core = make_core_with_limits(mock_provider_response()?, limits, 30, 8192).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn request_below_broker_default_with_no_tenant_override_is_granted() -> TestResult {
    let limits = Arc::new(InMemoryTenantLimits::new());
    let core = make_core_with_limits(mock_provider_response()?, limits, 3600, 8192).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn passes_cpu_but_fails_memory_is_denied() -> TestResult {
    let limits = Arc::new(InMemoryTenantLimits::new());
    let core = make_core_with_limits(mock_provider_response()?, limits, 3600, 256).await?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}
