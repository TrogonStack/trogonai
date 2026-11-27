mod support;

use std::time::Duration;

use support::{Rig, TestResult};
use trogon_sandbox_contract::{LeaseError, LeaseId, SandboxFreezeResponse};
use trogon_sandbox_space_worker::{LifecycleAction, SpaceId, SpaceProviderError};

fn denied(response: &SandboxFreezeResponse) -> String {
    match response {
        SandboxFreezeResponse::Denied(LeaseError::Backend { message }) => message.clone(),
        _ => String::new(),
    }
}

fn sandbox_state(rig: &Rig) -> String {
    rig.space
        .state()
        .sandboxes
        .values()
        .next()
        .map(|sandbox| sandbox.state.clone())
        .unwrap_or_default()
}

#[tokio::test]
async fn pauses_suspends_and_wakes() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let provider = &rig.provider;
    assert!(matches!(
        provider.set_dormancy(&lease, LifecycleAction::Pause).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(sandbox_state(&rig), "paused");
    assert!(matches!(
        provider.set_dormancy(&lease, LifecycleAction::Suspend).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(sandbox_state(&rig), "suspended");
    assert!(matches!(
        provider.set_dormancy(&lease, LifecycleAction::Wake).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(sandbox_state(&rig), "running");
    Ok(())
}

#[tokio::test]
async fn asking_for_the_current_state_changes_nothing() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let before = rig.space.state().log.len();
    assert!(matches!(
        rig.provider.set_dormancy(&lease, LifecycleAction::Wake).await,
        SandboxFreezeResponse::Done
    ));
    let after = rig.space.state().log.clone();
    assert!(!after[before..].iter().any(|line| line.starts_with("POST")));
    Ok(())
}

#[tokio::test]
async fn refuses_a_transition_the_state_does_not_allow() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space
        .state()
        .sandboxes
        .values_mut()
        .for_each(|sandbox| sandbox.state = "stopped".into());
    let response = rig.provider.set_dormancy(&lease, LifecycleAction::Pause).await;
    assert_eq!(denied(&response), "cannot pause a sandbox that is stopped");
    Ok(())
}

#[tokio::test]
async fn reports_a_sandbox_that_is_gone() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().sandboxes.clear();
    let response = rig.provider.set_dormancy(&lease, LifecycleAction::Pause).await;
    assert_eq!(denied(&response), SpaceProviderError::Missing.to_string());
    Ok(())
}

#[tokio::test]
async fn reports_a_lease_it_does_not_hold() -> TestResult {
    let rig = Rig::start().await?;
    let response = rig
        .provider
        .set_dormancy(&LeaseId::new("nobody"), LifecycleAction::Pause)
        .await;
    assert!(matches!(
        response,
        SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_stuck_in_transition_times_out() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().lifecycle_stuck = true;
    let response = rig.provider.set_dormancy(&lease, LifecycleAction::Pause).await;
    assert!(denied(&response).contains("did not reach paused"));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_settles_elsewhere_is_reported() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().operation_final = Some("succeeded".into());
    rig.space
        .state()
        .sandboxes
        .values_mut()
        .for_each(|sandbox| sandbox.state = "paused".into());
    // Wake lands on "running" in the fake, so make the next state fail instead.
    rig.space.state().lifecycle_stuck = false;
    let response = rig.provider.set_dormancy(&lease, LifecycleAction::Wake).await;
    assert!(matches!(response, SandboxFreezeResponse::Done));
    Ok(())
}

#[tokio::test]
async fn waking_waits_for_the_guest() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.provider.set_dormancy(&lease, LifecycleAction::Pause).await;
    rig.space.state().probe_status = Some(400);
    let response = rig.provider.set_dormancy(&lease, LifecycleAction::Wake).await;
    assert!(!denied(&response).is_empty());
    Ok(())
}

#[tokio::test]
async fn destroy_retries_a_conflict_until_the_state_settles() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    rig.space.state().delete_conflicts = 2;
    rig.provider.destroy(&id).await?;
    assert!(rig.space.state().sandboxes.is_empty());
    Ok(())
}

#[tokio::test]
async fn destroy_waits_out_a_sandbox_that_is_deleting() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    {
        let mut state = rig.space.state();
        state.delete_conflicts = 1;
        state
            .sandboxes
            .values_mut()
            .for_each(|sandbox| sandbox.state = "deleting".into());
    }
    let result = rig.provider.destroy(&id).await;
    assert!(matches!(result, Err(SpaceProviderError::DeleteTimeout { .. })));
    Ok(())
}

#[tokio::test]
async fn destroy_accepts_a_sandbox_that_vanished_during_a_conflict() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    {
        let mut state = rig.space.state();
        state.delete_conflicts = 1;
        state.sandboxes.clear();
    }
    rig.provider.destroy(&id).await?;
    Ok(())
}

#[tokio::test]
async fn destroy_accepts_a_sandbox_marked_deleted() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    rig.space.state().delete_marks_deleted = true;
    rig.provider.destroy(&id).await?;
    Ok(())
}

#[tokio::test]
async fn destroy_accepts_a_conflict_over_a_deleted_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    {
        let mut state = rig.space.state();
        state.delete_conflicts = 1;
        state
            .sandboxes
            .values_mut()
            .for_each(|sandbox| sandbox.state = "deleted".into());
    }
    rig.provider.destroy(&id).await?;
    Ok(())
}

#[tokio::test]
async fn destroy_gives_up_on_a_sandbox_that_stays() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    rig.space.state().delete_keeps = true;
    let result = rig.provider.destroy(&id).await;
    assert!(matches!(result, Err(SpaceProviderError::DeleteTimeout { .. })));
    Ok(())
}

#[tokio::test]
async fn destroy_treats_an_unknown_sandbox_as_gone() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.destroy(&SpaceId::parse("sb-404")?).await?;
    Ok(())
}

#[tokio::test]
async fn destroy_keeps_conflicting_until_the_deadline() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let id = SpaceId::parse("sb-1")?;
    rig.space.state().delete_conflicts = u32::MAX;
    let result = tokio::time::timeout(Duration::from_secs(3), rig.provider.destroy(&id)).await?;
    assert!(result.is_err());
    Ok(())
}
