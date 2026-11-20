use super::*;
use crate::{ProtocolError, SignalNumber, SignalRequestId};

#[expect(
    clippy::expect_used,
    reason = "A fixed valid identity is required to arrange the private state contract"
)]
fn supervisor() -> Supervisor {
    Supervisor::new(
        ExecutionId::new("state-contract").expect("valid fixed identity"),
        MAX_CHILDREN,
    )
}

#[test]
fn a_control_command_cannot_be_prepared_as_a_workload() {
    let supervisor = supervisor();
    let command = Command::Release {
        execution_id: supervisor.execution,
    };

    assert!(matches!(
        Prepared::from_command(command),
        Err(ErrorCode::InvalidCommand)
    ));
}

fn fill_control_queue(supervisor: &mut Supervisor) -> Result<(), ProtocolError> {
    let event = Event::Status {
        execution_id: supervisor.execution.clone(),
        phase: supervisor.phase,
    };
    let frames = QUEUED_CONTROL_LIMIT / encode_event(&event)?.len();
    for _ in 0..frames {
        assert!(supervisor.writer.queue(event.clone()).is_ok());
    }
    assert!(matches!(supervisor.writer.queue(event), Err(ErrorCode::IoFailure)));
    Ok(())
}

fn signal(supervisor: &Supervisor, request_id: u64, signal: i32) -> Result<Command, ProtocolError> {
    Ok(Command::Signal {
        execution_id: supervisor.execution.clone(),
        request_id: SignalRequestId::new(request_id)?,
        signal: SignalNumber::new(signal)?,
    })
}

#[test]
fn failed_error_delivery_closes_control_and_retains_drain() -> Result<(), ProtocolError> {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Running;
    fill_control_queue(&mut supervisor)?;
    let queued = supervisor.writer.pending.len();

    supervisor.error(ErrorCode::CleanupPending, true);

    assert!(supervisor.writer.closed);
    assert!(matches!(supervisor.phase, Phase::Draining));
    assert!(supervisor.canceled);
    assert!(supervisor.cleanup_started.is_some());
    assert_eq!(supervisor.writer.pending.len(), queued);
    assert!(!supervisor.acknowledged);
    Ok(())
}

#[test]
fn failed_signal_receipt_still_initiates_cleanup() -> Result<(), ProtocolError> {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Running;
    fill_control_queue(&mut supervisor)?;
    let queued = supervisor.writer.pending.len();

    supervisor.signal_result(1, libc::SIGTERM, Ok(true));

    assert!(matches!(supervisor.phase, Phase::Draining));
    assert!(supervisor.canceled);
    assert!(supervisor.cleanup_started.is_some());
    assert_eq!(supervisor.writer.pending.len(), queued);
    assert!(!supervisor.writer.closed);
    Ok(())
}

#[test]
fn confirmed_absence_remains_terminal_when_cleanup_is_repeated() {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Absent;
    let terminal_at = Instant::now();
    supervisor.terminal_at = Some(terminal_at);

    supervisor.begin_drain();
    supervisor.begin_drain();

    assert!(matches!(supervisor.phase, Phase::Absent));
    assert!(!supervisor.canceled);
    assert!(supervisor.cleanup_started.is_none());
    assert_eq!(supervisor.terminal_at, Some(terminal_at));
    assert!(supervisor.writer.pending.is_empty());
}

#[test]
fn foreign_acknowledgment_cannot_retire_the_anchor() -> Result<(), crate::ExecutionIdError> {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Absent;

    supervisor.command(Command::Acknowledge {
        execution_id: ExecutionId::new("foreign-execution")?,
    });

    assert!(!supervisor.acknowledged);
    assert!(matches!(supervisor.phase, Phase::Absent));
    assert_eq!(supervisor.writer.pending.len(), 1);

    supervisor.command(Command::Acknowledge {
        execution_id: supervisor.execution.clone(),
    });
    assert!(supervisor.acknowledged);
    assert_eq!(supervisor.writer.pending.len(), 1);
    Ok(())
}

#[test]
fn release_before_initialization_cannot_advance_execution() {
    let mut supervisor = supervisor();

    supervisor.command(Command::Release {
        execution_id: supervisor.execution.clone(),
    });

    assert!(matches!(supervisor.phase, Phase::AwaitingInit));
    assert!(!supervisor.released);
    assert!(!supervisor.canceled);
    assert!(supervisor.command_deadline.is_none());
    assert!(supervisor.cleanup_started.is_none());
    assert_eq!(supervisor.writer.pending.len(), 1);
}

#[test]
fn missing_held_gate_cannot_be_reported_as_released() {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Held;

    supervisor.command(Command::Release {
        execution_id: supervisor.execution.clone(),
    });

    assert!(matches!(supervisor.phase, Phase::Draining));
    assert!(!supervisor.released);
    assert!(supervisor.canceled);
    assert!(supervisor.command_deadline.is_none());
    assert!(supervisor.cleanup_started.is_some());
    assert_eq!(supervisor.writer.pending.len(), 1);
}

#[test]
fn process_limit_notice_is_once_and_cleanup_clock_does_not_restart() {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Running;

    supervisor.process_limit();
    let cleanup_started = supervisor.cleanup_started;
    supervisor.process_limit();

    assert!(supervisor.process_limit_notice);
    assert!(matches!(supervisor.phase, Phase::Draining));
    assert!(supervisor.canceled);
    assert!(cleanup_started.is_some());
    assert_eq!(supervisor.cleanup_started, cleanup_started);
    assert_eq!(supervisor.writer.pending.len(), 1);
}

#[test]
fn replay_of_unsettled_or_failed_signal_keeps_original_outcome() -> Result<(), ProtocolError> {
    for outcome in [None, Some(Err(ErrorCode::IdentityUnknown))] {
        let mut supervisor = supervisor();
        supervisor.phase = Phase::Held;
        supervisor.signal_request = Some(SignalRequest {
            request_id: 7,
            signal: libc::SIGTERM,
            outcome,
        });

        supervisor.command(signal(&supervisor, 7, libc::SIGTERM)?);

        assert!(matches!(supervisor.phase, Phase::Draining));
        assert!(supervisor.canceled);
        assert!(supervisor.cleanup_started.is_some());
        assert_eq!(supervisor.writer.pending.len(), 1);
        assert!(supervisor.signal_request.is_some_and(|request| {
            request.request_id == 7 && request.signal == libc::SIGTERM && request.outcome == outcome
        }));
    }
    Ok(())
}

#[test]
fn conflicting_or_older_signal_replay_cannot_replace_the_original() -> Result<(), ProtocolError> {
    for (request_id, requested_signal) in [(6, libc::SIGTERM), (7, libc::SIGINT)] {
        let mut supervisor = supervisor();
        supervisor.phase = Phase::Held;
        supervisor.signal_request = Some(SignalRequest {
            request_id: 7,
            signal: libc::SIGTERM,
            outcome: Some(Ok(true)),
        });

        supervisor.command(signal(&supervisor, request_id, requested_signal)?);

        assert!(matches!(supervisor.phase, Phase::Held));
        assert!(!supervisor.canceled);
        assert!(supervisor.cleanup_started.is_none());
        assert_eq!(supervisor.writer.pending.len(), 1);
        assert!(supervisor.signal_request.is_some_and(|request| {
            request.request_id == 7 && request.signal == libc::SIGTERM && request.outcome == Some(Ok(true))
        }));
    }
    Ok(())
}

#[test]
fn signal_after_absence_reports_no_delivery_without_reopening_cleanup() -> Result<(), ProtocolError> {
    let mut supervisor = supervisor();
    supervisor.phase = Phase::Absent;

    supervisor.command(signal(&supervisor, 1, libc::SIGKILL)?);

    assert!(matches!(supervisor.phase, Phase::Absent));
    assert!(!supervisor.canceled);
    assert!(supervisor.cleanup_started.is_none());
    assert_eq!(supervisor.writer.pending.len(), 2);
    assert!(supervisor.signal_request.is_some_and(|request| {
        request.request_id == 1 && request.signal == libc::SIGKILL && request.outcome == Some(Ok(false))
    }));
    Ok(())
}
