use super::*;
use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_protocol::{
    DispatchDecodeError, DispatchSigner, DispatchVerifier, EnforcementMode, OP_LEASE_DISPATCH,
};

#[tokio::test]
async fn native_options_are_denied_for_a_tenant_without_the_policy() -> TestResult {
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        false,
    )?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.native = Some(serde_json::json!({"mock": {}}));
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::NativePassthroughNotPermitted { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn native_options_need_a_provider_keyed_object() -> TestResult {
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    for native in [serde_json::json!({"not_a_vendor": {}}), serde_json::json!(["mock"])] {
        let mut req = make_request(vec![ProviderKind::Mock])?;
        req.native = Some(native);
        assert!(matches!(
            core.handle_lease_request(req).await,
            LeaseResponse::Denied(LeaseError::InvalidNativeOptions { .. })
        ));
    }
    Ok(())
}

#[tokio::test]
async fn dispatch_carries_only_the_chosen_providers_native_options() -> TestResult {
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let core = placement_core(
        bus.clone(),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.native = Some(serde_json::json!({
        "mock": {"knob": 1},
        "daytona": {"public": true}
    }));
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Granted(_)
    ));
    assert_eq!(
        captured_dispatch(&bus)?.request.native,
        Some(serde_json::json!({"mock": {"knob": 1}}))
    );
    Ok(())
}

#[tokio::test]
async fn a_signed_dispatch_reaches_a_worker_verifier_and_a_tamper_does_not() -> TestResult {
    let signer = DispatchSigner::new("k1", [7u8; 32]);
    let public = signer.public_key_b64();
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let core = placement_core(
        bus.clone(),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?
    .with_dispatch_signer(Arc::new(signer));
    let req = make_request(vec![ProviderKind::Mock])?;
    let tenant = req.tenant.clone();
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Granted(_)
    ));
    let sent = locked(&bus.last_payload)?
        .clone()
        .ok_or("no dispatch payload was captured")?;

    let require = DispatchVerifier::new(EnforcementMode::Require).with_key("k1", &public)?;
    let dispatch: trogon_sandbox_contract::ProviderDispatchPayload = require.decode(&sent, OP_LEASE_DISPATCH, None)?;
    assert_eq!(dispatch.request.tenant, tenant);

    let mut forged: serde_json::Value = serde_json::from_slice(&sent)?;
    forged["request"]["tenant"] = "someone-else".into();
    let forged = serde_json::to_vec(&forged)?;
    let fresh = DispatchVerifier::new(EnforcementMode::Require).with_key("k1", &public)?;
    assert!(matches!(
        fresh.decode::<trogon_sandbox_contract::ProviderDispatchPayload>(&forged, OP_LEASE_DISPATCH, None),
        Err(DispatchDecodeError::Auth(_))
    ));
    Ok(())
}

#[tokio::test]
async fn native_options_only_reach_workers_that_declare_passthrough() -> TestResult {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_placement_worker(&registry, ProviderKind::Daytona, "plain", &[], false).await?;
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        registry.clone(),
        true,
        true,
    )?;
    let mut req = make_request(vec![ProviderKind::Daytona])?;
    req.native = Some(serde_json::json!({"daytona": {"public": true}}));
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
    ));

    inject_placement_worker(&registry, ProviderKind::Modal, "open", &[], true).await?;
    let mut req = make_request(vec![ProviderKind::Daytona, ProviderKind::Modal])?;
    req.idempotency_key = LeaseRequestKey::new("native-2")?;
    req.native = Some(serde_json::json!({"daytona": {"public": true}}));
    match core.handle_lease_request(req).await {
        LeaseResponse::Granted(granted) => assert_eq!(granted.provider, ProviderKind::Modal),
        other => return Err(format!("expected a grant on the passthrough worker, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn a_restore_is_placed_on_the_worker_with_the_matching_environment() -> TestResult {
    let digest = |c: char| EnvDigest::new(format!("sha256:{}", c.to_string().repeat(64)));
    let digest_a = digest('a')?;
    let digest_b = digest('b')?;
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_placement_worker(&registry, ProviderKind::Daytona, "old", &[], false).await?;
    // A second worker advertising the digest the snapshot was taken under.
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::Modal;
    profile.placement.env_digest = Some(digest_a.clone());
    registry
        .inject(worker_heartbeat("matching", ProviderKind::Modal, profile)?)
        .await;
    // The gate holds with the worker registry unenforced too.
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        registry,
        false,
        false,
    )?;
    let wants = |digest: &EnvDigest| CapabilityRequirements {
        env_digest: Some(digest.clone()),
        ..Default::default()
    };
    let mut req = make_request(vec![ProviderKind::Daytona, ProviderKind::Modal])?;
    req.required_capabilities = Some(wants(&digest_a));
    match core.handle_lease_request(req).await {
        LeaseResponse::Granted(granted) => assert_eq!(granted.provider, ProviderKind::Modal),
        other => {
            return Err(format!("expected a grant on the matching worker, got {other:?}").into());
        }
    }
    let mut req = make_request(vec![ProviderKind::Daytona, ProviderKind::Modal])?;
    req.idempotency_key = LeaseRequestKey::new("env-2")?;
    req.required_capabilities = Some(wants(&digest_b));
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn region_filters_workers_even_when_the_registry_is_not_enforced() -> TestResult {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_placement_worker(&registry, ProviderKind::Daytona, "eu", &["eu"], false).await?;
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        registry,
        false,
        false,
    )?;
    let mut req = make_request(vec![ProviderKind::Daytona])?;
    req.region = Some("us".to_string());
    match core.handle_lease_request(req).await {
        LeaseResponse::Denied(LeaseError::NoWorkerInRegion { regions }) => {
            assert_eq!(regions, vec!["us".to_string()]);
        }
        other => return Err(format!("expected NoWorkerInRegion, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn fallback_regions_are_tried_in_order_and_the_match_is_dispatched() -> TestResult {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_placement_worker(&registry, ProviderKind::Daytona, "ap", &["ap"], false).await?;
    inject_placement_worker(&registry, ProviderKind::Modal, "eu", &["eu"], false).await?;
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let core = placement_core(bus.clone(), registry, true, false)?;
    let mut req = make_request(vec![ProviderKind::Daytona, ProviderKind::Modal])?;
    req.region = Some("us".to_string());
    req.fallback_regions = vec!["eu".to_string(), "ap".to_string()];
    match core.handle_lease_request(req).await {
        LeaseResponse::Granted(granted) => assert_eq!(granted.provider, ProviderKind::Modal),
        other => return Err(format!("expected the first fallback to win, got {other:?}").into()),
    }
    let dispatch = captured_dispatch(&bus)?;
    assert_eq!(dispatch.request.region.as_deref(), Some("eu"));
    assert!(dispatch.request.fallback_regions.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_region_miss_is_not_reported_when_another_requirement_fails_too() -> TestResult {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        registry,
        true,
        false,
    )?;
    let mut req = make_request(vec![ProviderKind::Daytona])?;
    req.region = Some("us".to_string());
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::NoProviderAvailable)
    ));
    Ok(())
}

#[tokio::test]
async fn fallback_regions_without_a_region_are_rejected() -> TestResult {
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        false,
    )?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.fallback_regions = vec!["eu".to_string()];
    assert!(matches!(
        core.handle_lease_request(req).await,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn worker_native_rejection_maps_to_invalid_native_options() -> TestResult {
    let reply = Bytes::from(serde_json::to_vec(
        &serde_json::json!({"native_error": "unknown native option 'x'"}),
    )?);
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: reply,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.native = Some(serde_json::json!({"mock": {"x": 1}}));
    match core.handle_lease_request(req).await {
        LeaseResponse::Denied(LeaseError::InvalidNativeOptions { provider, reason }) => {
            assert_eq!(provider, ProviderKind::Mock);
            assert!(reason.contains("'x'"));
        }
        other => return Err(format!("expected InvalidNativeOptions, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn native_handle_requires_the_tenant_policy() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: native_handle_reply()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        false,
    )?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert!(matches!(
        core.handle_native_handle(NativeHandleRequest { lease_id }).await,
        NativeHandleResponse::Denied(LeaseError::NativePassthroughNotPermitted { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn native_handle_returns_the_workers_vendor_identity() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: native_handle_reply()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    match core.handle_native_handle(NativeHandleRequest { lease_id }).await {
        NativeHandleResponse::Available(handle) => assert_eq!(handle.vendor_id, "vendor-1"),
        other => return Err(format!("expected a handle, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn ssh_access_requires_the_tenant_policy() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: ssh_access_reply()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        false,
    )?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert!(matches!(
        core.handle_ssh_access(SshAccessRequest {
            lease_id,
            ttl_seconds: None
        })
        .await,
        SshAccessResponse::Denied(LeaseError::NativePassthroughNotPermitted { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn ssh_access_relays_the_workers_endpoint() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: ssh_access_reply()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    match core
        .handle_ssh_access(SshAccessRequest {
            lease_id,
            ttl_seconds: Some(600),
        })
        .await
    {
        SshAccessResponse::Available(access) => {
            assert_eq!(access.host, "ssh.example");
            assert_eq!(access.user, "token-1");
        }
        other => return Err(format!("expected ssh access, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn ssh_access_is_denied_for_an_unknown_lease() -> TestResult {
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: ssh_access_reply()?,
        }),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    assert!(matches!(
        core.handle_ssh_access(SshAccessRequest {
            lease_id: LeaseId::new("missing"),
            ttl_seconds: None
        })
        .await,
        SshAccessResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn ssh_access_is_unsupported_when_no_worker_serves_it() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = placement_core(
        Arc::new(NoRespondersBus),
        InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30)),
        false,
        true,
    )?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert_eq!(
        core.handle_ssh_access(SshAccessRequest {
            lease_id,
            ttl_seconds: None
        })
        .await,
        SshAccessResponse::Unsupported
    );
    Ok(())
}

#[tokio::test]
async fn native_handle_is_unsupported_without_a_passthrough_worker() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_placement_worker(&registry, ProviderKind::Mock, "plain", &[], false).await?;
    let core = placement_core(
        Arc::new(MockBus {
            provider_response: native_handle_reply()?,
        }),
        registry,
        true,
        true,
    )?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert_eq!(
        core.handle_native_handle(NativeHandleRequest { lease_id }).await,
        NativeHandleResponse::Unsupported
    );
    Ok(())
}
