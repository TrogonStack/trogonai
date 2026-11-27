use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, WorkerId};
use trogon_sandbox_protocol::lease_epoch_for_assignment;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn epoch_is_minted_once_and_bumped_when_the_assignment_changes() -> TestResult {
    let w1 = WorkerId::new("w1")?;
    let w2 = WorkerId::new("w2")?;
    let a1 = DispatchAttemptId::new("a1")?;
    let a2 = DispatchAttemptId::new("a2")?;
    let first = lease_epoch_for_assignment(None, &w1, &a1);
    assert_eq!(first, LeaseEpoch::FIRST);
    let resend = lease_epoch_for_assignment(Some((&w1, &a1, Some(first))), &w1, &a1);
    assert_eq!(resend, first);
    let moved = lease_epoch_for_assignment(Some((&w1, &a1, Some(first))), &w2, &a2);
    assert_eq!(moved, LeaseEpoch::new(2)?);
    let retried = lease_epoch_for_assignment(Some((&w1, &a1, Some(first))), &w1, &a2);
    assert_eq!(retried, LeaseEpoch::new(2)?);
    let legacy = lease_epoch_for_assignment(Some((&w1, &a1, None)), &w1, &a1);
    assert_eq!(legacy, LeaseEpoch::FIRST);
    Ok(())
}

#[test]
fn a_changed_worker_alone_or_attempt_alone_bumps_the_epoch() -> TestResult {
    let w1 = WorkerId::new("w1")?;
    let w2 = WorkerId::new("w2")?;
    let a1 = DispatchAttemptId::new("a1")?;
    let held = LeaseEpoch::new(5)?;
    let other_worker = lease_epoch_for_assignment(Some((&w1, &a1, Some(held))), &w2, &a1);
    assert_eq!(other_worker, LeaseEpoch::new(6)?);
    Ok(())
}

#[test]
fn a_prior_assignment_without_an_epoch_that_changes_starts_at_the_first_successor() -> TestResult {
    let w1 = WorkerId::new("w1")?;
    let a1 = DispatchAttemptId::new("a1")?;
    let a2 = DispatchAttemptId::new("a2")?;
    let next = lease_epoch_for_assignment(Some((&w1, &a1, None)), &w1, &a2);
    assert_eq!(next, LeaseEpoch::FIRST);
    Ok(())
}

#[test]
fn the_epoch_saturates_instead_of_wrapping() -> TestResult {
    let w1 = WorkerId::new("w1")?;
    let w2 = WorkerId::new("w2")?;
    let a1 = DispatchAttemptId::new("a1")?;
    let max = LeaseEpoch::new(u64::MAX)?;
    let next = lease_epoch_for_assignment(Some((&w1, &a1, Some(max))), &w2, &a1);
    assert_eq!(next, max);
    Ok(())
}
