mod support;

use support::{Rig, TestResult, dispatch};
use trogon_sandbox_space_worker::{ApiError, SpaceProviderError};

#[tokio::test]
async fn leases_a_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    assert!(rig.provider.holds_lease(&lease).await);
    assert_eq!(rig.provider.current_leases().await, 1);
    let state = rig.space.state();
    let body = state.create_bodies.first().ok_or("no create")?;
    assert_eq!(body["template"], "seed:latest");
    assert_eq!(body["metadata"]["sandbox_lease"], "lease-1");
    assert!(state.log.iter().any(|line| line.ends_with("/processes")));
    Ok(())
}

#[tokio::test]
async fn waits_for_slow_operations_and_boots() -> TestResult {
    let rig = Rig::start().await?;
    {
        let mut state = rig.space.state();
        state.operation_pending_polls = 3;
        state.boot_polls = 3;
    }
    rig.lease("lease-1").await?;
    Ok(())
}

#[tokio::test]
async fn trusts_a_terminal_operation_in_the_create_reply() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().embed_terminal_operation = true;
    rig.lease("lease-1").await?;
    assert!(
        !rig.space
            .state()
            .log
            .iter()
            .any(|line| line.contains("/v1/operations/"))
    );
    Ok(())
}

#[tokio::test]
async fn a_rejected_create_is_not_reconciled() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(400);
    let error = rig.lease("lease-1").await.err().ok_or("leased")?;
    assert!(!format!("{error}").is_empty());
    assert_eq!(rig.space.state().create_bodies.len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_unavailable_create_is_retried_then_reconciled() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(503);
    assert!(rig.lease("lease-1").await.is_err());
    assert!(rig.space.state().create_bodies.len() > 1);
    Ok(())
}

#[tokio::test]
async fn a_create_that_fails_to_reconcile_says_so() -> TestResult {
    let rig = Rig::start().await?;
    {
        let mut state = rig.space.state();
        state.create_status = Some(503);
        state.list_status = Some(400);
    }
    let payload = dispatch("lease-1", None)?;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::Reconcile(_)));
    Ok(())
}

#[tokio::test]
async fn a_create_reply_without_an_id_is_reconciled_away() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_without_id = true;
    let error = rig.lease("lease-1").await.err().ok_or("leased")?;
    assert!(!format!("{error}").is_empty());
    assert!(rig.space.state().sandboxes.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_failed_operation_deletes_the_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().operation_final = Some("failed".into());
    let error = rig.lease("lease-1").await.err().ok_or("leased")?;
    assert!(!format!("{error}").is_empty());
    assert!(rig.space.state().sandboxes.is_empty());
    assert_eq!(rig.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_operation_and_a_failed_delete_are_both_reported() -> TestResult {
    let rig = Rig::start().await?;
    {
        let mut state = rig.space.state();
        state.operation_final = Some("failed".into());
        state.delete_status = Some(400);
    }
    let payload = dispatch("lease-1", None)?;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::CleanupFailed { .. }));
    assert!(!format!("{error}").is_empty());
    Ok(())
}

#[tokio::test]
async fn an_operation_that_never_finishes_times_out() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().operation_pending_polls = u32::MAX;
    let payload = dispatch("lease-1", None)?;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::OperationTimeout { .. }));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_never_boots_times_out() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().boot_polls = u32::MAX;
    let payload = dispatch("lease-1", None)?;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::StateTimeout { .. }));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_settles_elsewhere_is_rejected() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_state = Some("failed".into());
    let payload = dispatch("lease-1", None)?;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::UnexpectedState { .. }));
    assert!(!format!("{error}").is_empty());
    Ok(())
}

#[tokio::test]
async fn a_guest_that_does_not_answer_fails_the_create() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().probe_status = Some(400);
    assert!(rig.lease("lease-1").await.is_err());
    assert!(rig.space.state().sandboxes.is_empty());
    rig.space.state().probe_status = None;
    rig.space.state().probe_malformed = true;
    assert!(rig.lease("lease-2").await.is_err());
    Ok(())
}

#[tokio::test]
async fn refuses_a_lease_held_or_beyond_capacity() -> TestResult {
    let rig = Rig::with(1).await?;
    rig.lease("lease-1").await?;
    let error = rig.lease("lease-1").await.err().ok_or("leased twice")?;
    assert!(matches!(
        error.downcast_ref::<SpaceProviderError>(),
        Some(SpaceProviderError::Busy)
    ));
    let error = rig.lease("lease-2").await.err().ok_or("over capacity")?;
    assert!(!format!("{error}").is_empty());
    Ok(())
}

#[tokio::test]
async fn api_errors_surface_their_code() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(400);
    let payload = dispatch("lease-1", None)?;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::Api(ApiError::Status { .. })));
    Ok(())
}
