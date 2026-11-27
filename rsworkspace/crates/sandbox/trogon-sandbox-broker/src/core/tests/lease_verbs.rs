use super::mounts_and_network_policy::bad_regex_policy;
use super::*;

use trogon_sandbox_contract::{NetworkPolicy, SandboxUserName, StopSignal, UnixSignal};

#[tokio::test]
async fn network_policy_update_denied_when_policy_is_invalid() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, bad_regex_policy()?))
        .await;

    assert!(
        matches!(
            response,
            NetworkPolicyUpdateResponse::Denied(LeaseError::InvalidNetworkPolicy { .. })
        ),
        "expected InvalidNetworkPolicy, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn network_policy_update_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(
            &LeaseId::new("missing"),
            NetworkPolicy::DenyAll,
        ))
        .await;

    assert!(matches!(
        response,
        NetworkPolicyUpdateResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn network_policy_update_denied_when_lease_not_running() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(mock_provider_response()?)?;
    let mut record = running_source_record(&tenant, &lease_id)?;
    record.status = LeaseStatus::Released;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, NetworkPolicy::DenyAll))
        .await;

    assert!(matches!(
        response,
        NetworkPolicyUpdateResponse::Denied(LeaseError::LeaseNotRunning { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn network_policy_update_applies_and_persists_new_policy() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core(network_policy_applied_response()?)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_network_policy_update(make_network_policy_update_request(&lease_id, NetworkPolicy::DenyAll))
        .await;

    assert!(
        matches!(response, NetworkPolicyUpdateResponse::Applied),
        "expected Applied, got {response:?}"
    );

    let updated = core
        .active_leases
        .get(&lease_id)
        .await?
        .ok_or("lease record should still exist after update")?;
    assert_eq!(updated.spec.network_policy(), Some(&NetworkPolicy::DenyAll));
    assert_eq!(updated.status, LeaseStatus::Running);
    Ok(())
}

#[tokio::test]
async fn create_user_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_create_user(CreateSandboxUserRequest {
            lease_id: LeaseId::new("missing"),
            username: SandboxUserName::new("agent-a")?,
        })
        .await;

    assert!(matches!(
        response,
        CreateSandboxUserResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn create_user_forwards_to_provider_and_returns_response() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let provider_response = Bytes::from(serde_json::to_vec(&CreateSandboxUserResponse::Created {
        home_dir: "/home/agent-a".to_string(),
    })?);
    let core = make_core(provider_response)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_create_user(CreateSandboxUserRequest {
            lease_id: lease_id.clone(),
            username: SandboxUserName::new("agent-a")?,
        })
        .await;

    assert_eq!(
        response,
        CreateSandboxUserResponse::Created {
            home_dir: "/home/agent-a".to_string(),
        }
    );
    Ok(())
}

#[tokio::test]
async fn kill_command_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;

    let response = core
        .handle_kill_command(KillCommandRequest {
            lease_id: LeaseId::new("missing"),
            execution_id: ExecutionId::new("exec-1"),
            signal: UnixSignal::Term,
        })
        .await;

    assert!(matches!(
        response,
        KillCommandResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn kill_command_forwards_to_provider_and_returns_response() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let provider_response = Bytes::from(serde_json::to_vec(&KillCommandResponse::Signaled)?);
    let core = make_core(provider_response)?;
    let record = running_source_record(&tenant, &lease_id)?;
    core.active_leases.put(&record).await?;

    let response = core
        .handle_kill_command(KillCommandRequest {
            lease_id: lease_id.clone(),
            execution_id: ExecutionId::new("exec-1"),
            signal: UnixSignal::Term,
        })
        .await;

    assert_eq!(response, KillCommandResponse::Signaled);
    Ok(())
}

#[tokio::test]
async fn stop_and_restart_denied_when_lease_not_found() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let lease_id = LeaseId::new("missing");

    assert!(matches!(
        core.handle_stop_command(stop_request(&lease_id)).await,
        StopCommandResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    assert!(matches!(
        core.handle_restart_command(restart_request(&lease_id)).await,
        RestartCommandResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn stop_and_restart_forward_to_provider_and_return_response() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let stop = Bytes::from(serde_json::to_vec(&StopCommandResponse::Stopped {
        signal: StopSignal::Kill,
    })?);
    let core = make_core(stop)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert_eq!(
        core.handle_stop_command(stop_request(&lease_id)).await,
        StopCommandResponse::Stopped {
            signal: StopSignal::Kill
        }
    );

    let restart = Bytes::from(serde_json::to_vec(&RestartCommandResponse::Restarted {
        execution_id: ExecutionId::new("exec-2"),
    })?);
    let core = make_core(restart)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    assert_eq!(
        core.handle_restart_command(restart_request(&lease_id)).await,
        RestartCommandResponse::Restarted {
            execution_id: ExecutionId::new("exec-2")
        }
    );
    Ok(())
}

#[tokio::test]
async fn stop_and_restart_are_unsupported_when_provider_lacks_the_capability() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-under-test");
    let core = make_core_enforcing_empty_registry(mock_provider_response()?)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    assert_eq!(
        core.handle_stop_command(stop_request(&lease_id)).await,
        StopCommandResponse::Unsupported
    );
    assert_eq!(
        core.handle_restart_command(restart_request(&lease_id)).await,
        RestartCommandResponse::Unsupported
    );
    Ok(())
}
