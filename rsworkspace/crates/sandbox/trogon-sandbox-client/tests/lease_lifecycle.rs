mod common;
mod lease_support;

use common::{MockBus, TestResult, client_replying, lease_id, tenant};
use lease_support::unix;
use std::time::Duration;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    ArtifactType, DeleteLeaseResponse, LeaseError, OciArtifactRef, OperationId, PauseLeaseResponse,
    ResumeLeaseResponse, RollbackLeaseResponse, StopLeaseResponse, SuspendLeaseResponse, SuspendLevel,
    SuspendRequestLevel, WakeLeaseResponse,
};

fn not_found() -> LeaseError {
    LeaseError::LeaseNotFound { lease_id: lease_id() }
}

#[tokio::test]
async fn delete_lease_succeeds_when_broker_deletes() -> TestResult {
    let (client, log) = client_replying(&DeleteLeaseResponse::Deleted { operation_id: None })?;
    client.delete_lease(tenant()?, &lease_id()).await?;
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.delete.lease-1");
    assert_eq!(sent.timeout, Duration::from_secs(2));
    Ok(())
}

#[tokio::test]
async fn delete_lease_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&DeleteLeaseResponse::Denied(not_found()))?;
    let result = client.delete_lease(tenant()?, &lease_id()).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn delete_lease_sends_the_operation_id_only_when_given() -> TestResult {
    let deleted = DeleteLeaseResponse::Deleted { operation_id: None };
    let (client, log) = client_replying(&deleted)?;
    client
        .delete_lease_with_operation(tenant()?, &lease_id(), Some(OperationId::new("del-1")?))
        .await?;
    assert_eq!(log.only()?.json()?["operation_id"], "del-1");

    let (client, log) = client_replying(&deleted)?;
    client.delete_lease(tenant()?, &lease_id()).await?;
    assert!(log.only()?.json()?.get("operation_id").is_none());
    Ok(())
}

#[tokio::test]
async fn stop_lease_succeeds_when_broker_stops() -> TestResult {
    let (client, _) = client_replying(&StopLeaseResponse::Stopped)?;
    client.stop_lease(tenant()?, &lease_id()).await?;
    Ok(())
}

#[tokio::test]
async fn stop_lease_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&StopLeaseResponse::Denied(not_found()))?;
    let result = client.stop_lease(tenant()?, &lease_id()).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn pause_lease_returns_paused_unsupported_and_denied() -> TestResult {
    let paused = PauseLeaseResponse::Paused {
        reap_at: unix(60),
        operation_id: None,
    };
    let (client, log) = client_replying(&paused)?;
    let out = client.pause_lease(tenant()?, &lease_id(), Some(60)).await?;
    assert_eq!(out, paused);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.pause.lease-1");
    assert_eq!(sent.timeout, Duration::from_secs(30));
    assert_eq!(sent.json()?["max_paused_seconds"], 60);

    let (client, _) = client_replying(&PauseLeaseResponse::Unsupported)?;
    let out = client.pause_lease(tenant()?, &lease_id(), None).await?;
    assert_eq!(out, PauseLeaseResponse::Unsupported);

    let (client, _) = client_replying(&PauseLeaseResponse::Denied(not_found()))?;
    let out = client.pause_lease(tenant()?, &lease_id(), None).await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn pause_lease_times_out_when_the_broker_never_answers() -> TestResult {
    let client = MockBus::timing_out().into_client();
    let out = client.pause_lease(tenant()?, &lease_id(), None).await;
    assert!(matches!(out, Err(ClientError::Timeout(t)) if t == Duration::from_secs(30)));
    Ok(())
}

#[tokio::test]
async fn suspend_and_wake_return_their_outcomes_and_denials() -> TestResult {
    let suspended = SuspendLeaseResponse::Suspended {
        level: SuspendLevel::Disk,
        reap_at: unix(60),
        operation_id: None,
    };
    let (client, log) = client_replying(&suspended)?;
    let out = client
        .suspend_lease(tenant()?, &lease_id(), SuspendRequestLevel::Auto, Some(60))
        .await?;
    assert_eq!(out, suspended);
    assert_eq!(log.only()?.subject, "sandbox.lease.suspend.lease-1");

    let denied = SuspendLeaseResponse::Denied(LeaseError::SuspendUnsupported {
        lease_id: lease_id(),
        requested: SuspendRequestLevel::Memory,
    });
    let (client, _) = client_replying(&denied)?;
    let out = client
        .suspend_lease(tenant()?, &lease_id(), SuspendRequestLevel::Memory, None)
        .await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::SuspendUnsupported { .. }))
    ));

    let woken = WakeLeaseResponse::Woken {
        deadline: unix(90),
        operation_id: None,
    };
    let (client, log) = client_replying(&woken)?;
    let out = client.wake_lease(tenant()?, &lease_id()).await?;
    assert_eq!(out, woken);
    assert_eq!(log.only()?.subject, "sandbox.lease.wake.lease-1");

    let waking = WakeLeaseResponse::Denied(LeaseError::LeaseWaking {
        lease_id: lease_id(),
        retry_after_ms: 1000,
    });
    let (client, _) = client_replying(&waking)?;
    let out = client.wake_lease(tenant()?, &lease_id()).await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::LeaseWaking {
            retry_after_ms: 1000,
            ..
        }))
    ));
    Ok(())
}

#[tokio::test]
async fn resume_lease_returns_resumed_and_denied() -> TestResult {
    let resumed = ResumeLeaseResponse::Resumed {
        deadline: unix(90),
        operation_id: None,
    };
    let (client, log) = client_replying(&resumed)?;
    let out = client.resume_lease(tenant()?, &lease_id()).await?;
    assert_eq!(out, resumed);
    assert_eq!(log.only()?.subject, "sandbox.lease.resume.lease-1");

    let denied = ResumeLeaseResponse::Denied(LeaseError::LeaseNotRunning { lease_id: lease_id() });
    let (client, _) = client_replying(&denied)?;
    let out = client.resume_lease(tenant()?, &lease_id()).await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::LeaseNotRunning { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn rollback_lease_returns_the_outcome_and_maps_denial() -> TestResult {
    let snapshot = OciArtifactRef {
        registry: "registry.example".into(),
        repository: "snapshots/one".into(),
        digest: format!("sha256:{}", "a".repeat(64)).parse()?,
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let (client, log) = client_replying(&RollbackLeaseResponse::Unsupported)?;
    let out = client
        .rollback_lease(
            tenant()?,
            &lease_id(),
            snapshot.clone(),
            Some(OperationId::new("rb-1")?),
        )
        .await?;
    assert_eq!(out, RollbackLeaseResponse::Unsupported);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.rollback.lease-1");
    assert_eq!(sent.json()?["operation_id"], "rb-1");

    let (client, _) = client_replying(&RollbackLeaseResponse::Denied(not_found()))?;
    let out = client.rollback_lease(tenant()?, &lease_id(), snapshot, None).await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}
