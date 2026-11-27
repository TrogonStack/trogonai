use std::time::{Duration, UNIX_EPOCH};

use trogon_sandbox_contract::{LeaseError, ProcessState};

use super::TestResult;
use crate::exec::clock::Clock;
use crate::exec::deadline::Deadline;
use crate::exec::deadline_error::DeadlineError;
use crate::exec::terminal::Terminal;
use crate::snapshots::CapturedDeadline;

#[test]
fn a_host_clock_outside_the_supported_range_is_refused() -> TestResult {
    let before = UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .ok_or("the platform represents times before the epoch")?;
    assert!(matches!(Clock::unix_ms_at(before), Err(DeadlineError::HostClock(_))));
    let far = UNIX_EPOCH
        .checked_add(Duration::from_secs(u64::MAX / 1000 + 1))
        .ok_or("the platform represents far future times")?;
    assert!(matches!(Clock::unix_ms_at(far), Err(DeadlineError::ClockRange(_))));
    assert!(matches!(
        Clock::millis(Duration::MAX),
        Err(DeadlineError::ClockRange(_))
    ));
    assert_eq!(Clock::unix_ms_at(UNIX_EPOCH + Duration::from_secs(2))?, 2000);
    assert!(matches!(
        LeaseError::from(DeadlineError::BeforeIssuance),
        LeaseError::InvalidSpec { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn a_terminal_deadline_is_captured_with_its_exit_code() -> TestResult {
    let captured = Deadline::Terminal(5).capture().await?;
    assert!(matches!(captured, CapturedDeadline::Terminal { exit_code: 5 }));
    Ok(())
}

fn timeout_capture(timed_out: bool) -> Result<CapturedDeadline, DeadlineError> {
    let now = Clock::now_unix_ms()?;
    Ok(CapturedDeadline::Timeout {
        issued_unix_ms: now - 10,
        expires_unix_ms: now + 60_000,
        captured_unix_ms: now - 5,
        remaining_ms: 30_000,
        timed_out,
    })
}

async fn terminal_of(deadline: &Deadline) -> Option<Terminal> {
    match deadline {
        Deadline::Finite { terminal, .. } => Some(terminal.lock().await.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn a_restored_deadline_needs_proof_of_how_its_process_ended() -> TestResult {
    assert!(Deadline::restore(None, ProcessState::Running).is_err());
    assert!(matches!(
        Deadline::restore(None, ProcessState::Exited { exit_code: 3 })?,
        Deadline::Terminal(3)
    ));
    assert!(matches!(
        Deadline::restore(Some(&CapturedDeadline::Lease), ProcessState::Running)?,
        Deadline::Lease
    ));
    let terminal = CapturedDeadline::Terminal { exit_code: 4 };
    assert!(matches!(
        Deadline::restore(Some(&terminal), ProcessState::Exited { exit_code: 4 })?,
        Deadline::Terminal(4)
    ));
    assert!(Deadline::restore(Some(&terminal), ProcessState::Exited { exit_code: 5 }).is_err());
    assert!(Deadline::restore(Some(&terminal), ProcessState::Running).is_err());

    let timed_out = timeout_capture(true)?;
    assert!(Deadline::restore(Some(&timed_out), ProcessState::Running).is_err());
    let restored = Deadline::restore(Some(&timed_out), ProcessState::Exited { exit_code: 137 })?;
    assert!(matches!(terminal_of(&restored).await, Some(Terminal::TimedOut)));

    let natural = timeout_capture(false)?;
    let restored = Deadline::restore(Some(&natural), ProcessState::Exited { exit_code: 2 })?;
    assert!(matches!(terminal_of(&restored).await, Some(Terminal::Natural(2))));
    let restored = Deadline::restore(Some(&natural), ProcessState::Running)?;
    assert!(matches!(terminal_of(&restored).await, Some(Terminal::Pending)));
    Ok(())
}

#[test]
fn a_clock_issued_in_the_future_has_no_remaining_time() -> TestResult {
    let now = Clock::now_unix_ms()?;
    let clock = Clock {
        issued_unix_ms: now + 3_600_000,
        expires_unix_ms: now + 7_200_000,
        due: tokio::time::Instant::now(),
    };
    assert!(matches!(clock.remaining_ms(), Err(DeadlineError::BeforeIssuance)));
    Ok(())
}
