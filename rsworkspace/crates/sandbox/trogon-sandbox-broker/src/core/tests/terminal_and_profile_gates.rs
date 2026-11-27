use super::*;
use trogon_sandbox_contract::NetworkPolicy;

#[tokio::test]
async fn terminal_is_unsupported_when_the_provider_profile_lacks_it() -> TestResult {
    let response = terminal_with_profile_flag(
        false,
        open_op()?,
        TerminalResponse::Opened {
            terminal_id: trogon_sandbox_contract::TerminalId::new("t-1")?,
        },
    )
    .await?;
    assert_eq!(response, TerminalResponse::Unsupported);
    Ok(())
}

#[tokio::test]
async fn terminal_is_forwarded_when_the_provider_profile_declares_it() -> TestResult {
    let opened = TerminalResponse::Opened {
        terminal_id: trogon_sandbox_contract::TerminalId::new("t-1")?,
    };
    let response = terminal_with_profile_flag(true, open_op()?, opened.clone()).await?;
    assert_eq!(response, opened);
    Ok(())
}

#[tokio::test]
async fn network_policy_update_is_refused_when_the_profile_lacks_it() -> TestResult {
    let (core, lease_id) = core_with_isolation_flags(network_policy_applied_response()?, |i| {
        i.supports_network_policy_update = CapabilitySupport::Unsupported;
    })
    .await?;
    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, NetworkPolicy::DenyAll))
        .await;
    assert!(
        matches!(
            &response,
            NetworkPolicyUpdateResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities {
                requirements
            }) if requirements.needs_network_policy_update.is_required()
        ),
        "got {response:?}"
    );
    let record = core
        .active_leases
        .get(&lease_id)
        .await?
        .ok_or("the lease record is missing")?;
    assert_ne!(record.spec.network_policy(), Some(&NetworkPolicy::DenyAll));
    Ok(())
}

#[tokio::test]
async fn network_policy_update_is_forwarded_when_the_profile_declares_it() -> TestResult {
    let (core, lease_id) = core_with_isolation_flags(network_policy_applied_response()?, |i| {
        i.supports_network_policy_update = CapabilitySupport::Supported;
    })
    .await?;
    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, NetworkPolicy::DenyAll))
        .await;
    assert_eq!(response, NetworkPolicyUpdateResponse::Applied);
    Ok(())
}

#[tokio::test]
async fn group_verbs_are_refused_when_the_profile_lacks_groups() -> TestResult {
    let (core, lease_id) = core_with_isolation_flags(
        Bytes::from(serde_json::to_vec(&CreateSandboxGroupResponse::Created {
            shared_dir: "/shared/dev".into(),
        })?),
        |i| i.supports_sandbox_groups = CapabilitySupport::Unsupported,
    )
    .await?;
    let created = core
        .handle_create_group(CreateSandboxGroupRequest {
            lease_id: lease_id.clone(),
            groupname: trogon_sandbox_contract::SandboxGroupName::new("dev")?,
        })
        .await;
    assert!(
        matches!(
            &created,
            CreateSandboxGroupResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities {
                requirements
            }) if requirements.needs_sandbox_groups.is_required()
        ),
        "got {created:?}"
    );
    Ok(())
}

#[tokio::test]
async fn create_group_is_forwarded_when_the_profile_declares_groups() -> TestResult {
    let created = CreateSandboxGroupResponse::Created {
        shared_dir: "/shared/dev".into(),
    };
    let (core, lease_id) = core_with_isolation_flags(Bytes::from(serde_json::to_vec(&created)?), |i| {
        i.supports_sandbox_groups = CapabilitySupport::Supported
    })
    .await?;
    let response = core
        .handle_create_group(CreateSandboxGroupRequest {
            lease_id,
            groupname: trogon_sandbox_contract::SandboxGroupName::new("dev")?,
        })
        .await;
    assert_eq!(response, created);
    Ok(())
}

#[tokio::test]
async fn terminal_rejects_a_malformed_request_before_dispatch() {
    let malformed = serde_json::json!({ "op": "resize", "cols": 0, "rows": 24 });
    assert!(serde_json::from_value::<TerminalOp>(malformed).is_err());
}

#[tokio::test]
async fn terminal_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let response = core
        .handle_terminal(TerminalRequest {
            lease_id: LeaseId::new("missing"),
            terminal_id: trogon_sandbox_contract::TerminalId::new("t-1")?,
            op: open_op()?,
        })
        .await;
    assert!(matches!(
        response,
        TerminalResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn get_sandbox_domain_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_get_sandbox_domain(GetSandboxDomainRequest {
            lease_id: LeaseId::new("missing"),
            port: 8080,
            wait_ms: None,
        })
        .await;

    assert!(matches!(
        response,
        GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn get_lease_by_name_returns_not_found_when_unindexed() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_get_lease_by_name(GetLeaseByNameRequest {
            tenant: TenantId::new("test-tenant")?,
            name: trogon_sandbox_contract::LeaseName::new("never-set")?,
        })
        .await;

    assert_eq!(response, GetLeaseByNameResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn get_lease_by_name_returns_found_for_indexed_running_lease() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("named-lease");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.tags = SandboxTags::new([("role".to_string(), "preview".to_string())].into_iter().collect())?;
    core.active_leases.put(&record).await?;
    core.named_leases.set(&tenant, "my-lease", &lease_id).await?;

    let response = core
        .handle_get_lease_by_name(GetLeaseByNameRequest {
            tenant: tenant.clone(),
            name: trogon_sandbox_contract::LeaseName::new("my-lease")?,
        })
        .await;

    match response {
        GetLeaseByNameResponse::Found { row } => {
            assert_eq!(row.lease_id, lease_id);
            assert_eq!(row.tenant, tenant);
            assert_eq!(row.status, record.status);
            assert_eq!(row.name, Some(trogon_sandbox_contract::LeaseName::new("my-lease")?));
            let serialized = serde_json::to_value(row)?;
            assert_eq!(serialized["tags"]["role"], "preview");
            Ok(())
        }
        other => Err(format!("expected Found, got {other:?}").into()),
    }
}
