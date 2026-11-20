use super::tests::{TestResult, isolated_process as isolated, refuse_syscall};
use super::*;
use crate::{ProtocolError, SignalNumber, SignalRequestId};
use std::process::{Child, Command as Process, Stdio};

#[path = "tests/read_interruption.rs"]
mod read_interruption;

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

pub(in crate::linux) fn worker_a_control_command_cannot_be_prepared_as_a_workload() {
    let supervisor = supervisor();
    let command = Command::Release {
        execution_id: supervisor.execution,
    };

    assert!(matches!(
        Prepared::from_command(command),
        Err(ErrorCode::InvalidCommand)
    ));
}

#[test]
fn a_control_command_cannot_be_prepared_as_a_workload() {
    worker_a_control_command_cannot_be_prepared_as_a_workload()
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

pub(in crate::linux) fn worker_failed_error_delivery_closes_control_and_retains_drain() -> Result<(), ProtocolError> {
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
fn failed_error_delivery_closes_control_and_retains_drain() -> Result<(), ProtocolError> {
    worker_failed_error_delivery_closes_control_and_retains_drain()
}

pub(in crate::linux) fn worker_failed_signal_receipt_still_initiates_cleanup() -> Result<(), ProtocolError> {
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
fn failed_signal_receipt_still_initiates_cleanup() -> Result<(), ProtocolError> {
    worker_failed_signal_receipt_still_initiates_cleanup()
}

pub(in crate::linux) fn worker_confirmed_absence_remains_terminal_when_cleanup_is_repeated() {
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
fn confirmed_absence_remains_terminal_when_cleanup_is_repeated() {
    worker_confirmed_absence_remains_terminal_when_cleanup_is_repeated()
}

pub(in crate::linux) fn worker_foreign_acknowledgment_cannot_retire_the_anchor() -> Result<(), crate::ExecutionIdError>
{
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
fn foreign_acknowledgment_cannot_retire_the_anchor() -> Result<(), crate::ExecutionIdError> {
    worker_foreign_acknowledgment_cannot_retire_the_anchor()
}

pub(in crate::linux) fn worker_release_before_initialization_cannot_advance_execution() {
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
fn release_before_initialization_cannot_advance_execution() {
    worker_release_before_initialization_cannot_advance_execution()
}

pub(in crate::linux) fn worker_missing_held_gate_cannot_be_reported_as_released() {
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
fn missing_held_gate_cannot_be_reported_as_released() {
    worker_missing_held_gate_cannot_be_reported_as_released()
}

pub(in crate::linux) fn worker_process_limit_notice_is_once_and_cleanup_clock_does_not_restart() {
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
fn process_limit_notice_is_once_and_cleanup_clock_does_not_restart() {
    worker_process_limit_notice_is_once_and_cleanup_clock_does_not_restart()
}

pub(in crate::linux) fn worker_replay_of_unsettled_or_failed_signal_keeps_original_outcome() -> Result<(), ProtocolError>
{
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
fn replay_of_unsettled_or_failed_signal_keeps_original_outcome() -> Result<(), ProtocolError> {
    worker_replay_of_unsettled_or_failed_signal_keeps_original_outcome()
}

pub(in crate::linux) fn worker_conflicting_or_older_signal_replay_cannot_replace_the_original()
-> Result<(), ProtocolError> {
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
fn conflicting_or_older_signal_replay_cannot_replace_the_original() -> Result<(), ProtocolError> {
    worker_conflicting_or_older_signal_replay_cannot_replace_the_original()
}

pub(in crate::linux) fn worker_signal_after_absence_reports_no_delivery_without_reopening_cleanup()
-> Result<(), ProtocolError> {
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

#[test]
fn signal_after_absence_reports_no_delivery_without_reopening_cleanup() -> Result<(), ProtocolError> {
    worker_signal_after_absence_reports_no_delivery_without_reopening_cleanup()
}

struct StateGuard {
    state: Supervisor,
    owned: Vec<(i32, OwnedFd)>,
}

impl StateGuard {
    fn new() -> Self {
        Self {
            state: supervisor(),
            owned: Vec::new(),
        }
    }

    fn initialize(&mut self) -> TestResult {
        self.state.command(initialization(&self.state)?);
        if self.state.root.is_none() {
            return Err(io::Error::other("held root was not retained").into());
        }
        Ok(())
    }

    fn companion(&mut self, exits: bool) -> Result<Child, Box<dyn std::error::Error>> {
        let mut child = if exits {
            Process::new("/bin/true").spawn()?
        } else {
            Process::new("/bin/sh")
                .args(["-c", "read -r line"])
                .stdin(Stdio::piped())
                .spawn()?
        };
        let descriptor = match pidfd(child.id() as i32) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                let killed = child.kill();
                let waited = child.wait();
                killed?;
                waited?;
                return Err(error.into());
            }
        };
        self.owned.push((child.id() as i32, descriptor));
        Ok(child)
    }

    fn confirm_cleanup(&mut self) -> TestResult {
        let root = self.state.root.as_ref().map(|root| (root.pid, &root.pidfd));
        for (_, descriptor) in root.into_iter().chain(self.owned.iter().map(|(pid, fd)| (*pid, fd))) {
            signal_pidfd(descriptor, libc::SIGKILL)?;
        }
        for (pid, _) in root.into_iter().chain(self.owned.iter().map(|(pid, fd)| (*pid, fd))) {
            let mut status = 0;
            if unsafe { libc::waitpid(pid, &mut status, 0) } != pid {
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                if unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED) } < 0 {
                    return Err(io::Error::last_os_error().into());
                }
                assert_eq!(unsafe { info.si_pid() }, pid);
                assert!(matches!(
                    info.si_code,
                    libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED
                ));
            } else {
                assert!(RootStatus::from_raw(status).is_some());
            }
        }
        self.state.root.take();
        self.owned.clear();
        Ok(())
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        if let Some(root) = &self.state.root {
            let _ = signal_pidfd(&root.pidfd, libc::SIGKILL);
        }
        for (_, descriptor) in &self.owned {
            let _ = signal_pidfd(descriptor, libc::SIGKILL);
        }
        if let Some(root) = &self.state.root {
            reap_owned(root.pid);
        }
        for (pid, _) in &self.owned {
            reap_owned(*pid);
        }
    }
}

fn reap_owned(pid: i32) {
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } != pid {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED);
        }
    }
}

fn initialization(supervisor: &Supervisor) -> Result<Command, ProtocolError> {
    crate::CommandWire::Init {
        version: VERSION,
        execution_id: supervisor.execution.clone(),
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    }
    .try_into()
}

fn observe_exit(pid: i32) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { info.si_pid() } == pid {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("owned child did not reach a waitable exit").into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(in crate::linux) fn worker_unavailable_current_directory_cannot_create_a_held_root() -> TestResult {
    isolated(
        "linux::state_tests::unavailable_current_directory_cannot_create_a_held_root",
        || {
            let original = std::env::current_dir()?;
            let directory = tempfile::tempdir()?;
            std::env::set_current_dir(directory.path())?;
            std::fs::remove_dir(directory.path())?;
            let mut supervisor = supervisor();

            supervisor.command(initialization(&supervisor)?);
            std::env::set_current_dir(original)?;

            assert!(supervisor.root.is_none());
            assert!(matches!(supervisor.phase, Phase::AwaitingInit));
            assert!(!supervisor.released);
            assert_eq!(supervisor.writer.pending.len(), 1);
            Ok(())
        },
    )
}

#[test]
fn unavailable_current_directory_cannot_create_a_held_root() -> TestResult {
    worker_unavailable_current_directory_cannot_create_a_held_root()
}

pub(in crate::linux) fn worker_an_undeliverable_ready_receipt_keeps_the_root_held_and_drains() -> TestResult {
    isolated(
        "linux::state_tests::an_undeliverable_ready_receipt_keeps_the_root_held_and_drains",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            fill_control_queue(&mut guard.state)?;
            guard.initialize()?;

            assert!(matches!(guard.state.phase, Phase::Draining));
            assert!(guard.state.canceled);
            assert!(!guard.state.released);
            assert!(guard.state.cleanup_started.is_some());
            assert!(guard.state.root.as_ref().is_some_and(|root| root.gate.is_none()));
            Ok(())
        },
    )
}

#[test]
fn an_undeliverable_ready_receipt_keeps_the_root_held_and_drains() -> TestResult {
    worker_an_undeliverable_ready_receipt_keeps_the_root_held_and_drains()
}

pub(in crate::linux) fn worker_process_group_signal_refusal_does_not_claim_delivery() -> TestResult {
    isolated(
        "linux::state_tests::process_group_signal_refusal_does_not_claim_delivery",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            refuse_syscall(libc::SYS_kill, libc::EPERM)?;

            assert_eq!(guard.state.soft_signal(libc::SIGCONT), Err(ErrorCode::IdentityUnknown));
            assert!(!guard.state.released);
            assert!(matches!(guard.state.phase, Phase::Held));
            Ok(())
        },
    )
}

#[test]
fn process_group_signal_refusal_does_not_claim_delivery() -> TestResult {
    worker_process_group_signal_refusal_does_not_claim_delivery()
}

pub(in crate::linux) fn worker_incomplete_family_signal_scan_is_not_reported_as_complete() -> TestResult {
    isolated(
        "linux::state_tests::incomplete_family_signal_scan_is_not_reported_as_complete",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let _companion = guard.companion(false)?;
            guard.state.family_limit = 1;

            assert_eq!(guard.state.soft_signal(libc::SIGCONT), Err(ErrorCode::ProcessLimit));
            assert!(!guard.state.released);
            assert!(matches!(guard.state.phase, Phase::Held));
            Ok(())
        },
    )
}

#[test]
fn incomplete_family_signal_scan_is_not_reported_as_complete() -> TestResult {
    worker_incomplete_family_signal_scan_is_not_reported_as_complete()
}

pub(in crate::linux) fn worker_stdin_read_failure_closes_input_and_begins_cleanup() -> TestResult {
    isolated(
        "linux::state_tests::stdin_read_failure_closes_input_and_begins_cleanup",
        || {
            let saved = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 10) };
            if saved < 0 {
                return Err(io::Error::last_os_error().into());
            }
            let saved = unsafe { OwnedFd::from_raw_fd(saved) };
            assert_eq!(unsafe { libc::close(libc::STDIN_FILENO) }, 0);
            let mut supervisor = supervisor();

            supervisor.read_input();
            assert_eq!(unsafe { libc::dup2(saved.as_raw_fd(), libc::STDIN_FILENO) }, 0);

            assert!(supervisor.input_closed);
            assert!(supervisor.canceled);
            assert!(matches!(supervisor.phase, Phase::Draining));
            assert!(supervisor.cleanup_started.is_some());
            Ok(())
        },
    )
}

#[test]
fn stdin_read_failure_closes_input_and_begins_cleanup() -> TestResult {
    worker_stdin_read_failure_closes_input_and_begins_cleanup()
}

pub(in crate::linux) fn worker_failed_output_reads_mark_each_stream_truncated() -> TestResult {
    isolated(
        "linux::state_tests::failed_output_reads_mark_each_stream_truncated",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            for stream in [Stream::Stdout, Stream::Stderr] {
                let (_read, write) = pipe()?;
                let root = guard.state.root.as_mut().ok_or_else(|| io::Error::other("held root"))?;
                match stream {
                    Stream::Stdout => root.stdout = Some(write),
                    Stream::Stderr => root.stderr = Some(write),
                }
                guard.state.read_output(stream);
            }

            assert!(guard.state.writer.stdout_truncated);
            assert!(guard.state.writer.stderr_truncated);
            assert!(!guard.state.released);
            assert!(matches!(guard.state.phase, Phase::Held));
            Ok(())
        },
    )
}

#[test]
fn failed_output_reads_mark_each_stream_truncated() -> TestResult {
    worker_failed_output_reads_mark_each_stream_truncated()
}

pub(in crate::linux) fn worker_failed_exit_observation_cannot_publish_family_absence() -> TestResult {
    isolated(
        "linux::state_tests::failed_exit_observation_cannot_publish_family_absence",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let queued = guard.state.writer.pending.len();
            refuse_syscall(libc::SYS_waitid, libc::EIO)?;

            assert_eq!(guard.state.tick_family(), Err(ErrorCode::IdentityUnknown));
            assert!(matches!(guard.state.phase, Phase::Held));
            assert!(guard.state.terminal_at.is_none());
            assert_eq!(guard.state.writer.pending.len(), queued);
            Ok(())
        },
    )
}

#[test]
fn failed_exit_observation_cannot_publish_family_absence() -> TestResult {
    worker_failed_exit_observation_cannot_publish_family_absence()
}

pub(in crate::linux) fn worker_a_descendant_reap_failure_does_not_increment_settlement() -> TestResult {
    isolated(
        "linux::state_tests::a_descendant_reap_failure_does_not_increment_settlement",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let companion = guard.companion(true)?;
            observe_exit(companion.id() as i32)?;
            refuse_syscall(libc::SYS_wait4, libc::EIO)?;

            assert_eq!(guard.state.tick_family(), Err(ErrorCode::IdentityUnknown));
            assert_eq!(guard.state.reaped, 0);
            assert!(guard.state.terminal_at.is_none());
            assert!(matches!(guard.state.phase, Phase::Held));
            Ok(())
        },
    )
}

#[test]
fn a_descendant_reap_failure_does_not_increment_settlement() -> TestResult {
    worker_a_descendant_reap_failure_does_not_increment_settlement()
}

pub(in crate::linux) fn worker_a_root_reap_failure_does_not_confirm_cleanup() -> TestResult {
    isolated(
        "linux::state_tests::a_root_reap_failure_does_not_confirm_cleanup",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            guard.state.command(Command::Release {
                execution_id: guard.state.execution.clone(),
            });
            let root = guard
                .state
                .root
                .as_mut()
                .ok_or_else(|| io::Error::other("running root"))?;
            observe_exit(root.pid)?;
            root.status = Some(RootStatus {
                code: Some(0),
                signal: None,
            });
            refuse_syscall(libc::SYS_wait4, libc::EIO)?;

            assert_eq!(guard.state.tick_family(), Err(ErrorCode::IdentityUnknown));
            assert_eq!(guard.state.reaped, 0);
            assert!(guard.state.root.as_ref().is_some_and(|root| !root.reaped));
            assert!(guard.state.terminal_at.is_none());
            assert!(matches!(guard.state.phase, Phase::Running));
            Ok(())
        },
    )
}

#[test]
fn a_root_reap_failure_does_not_confirm_cleanup() -> TestResult {
    worker_a_root_reap_failure_does_not_confirm_cleanup()
}

pub(in crate::linux) fn worker_failed_global_exit_scan_does_not_confirm_an_empty_family() -> TestResult {
    isolated(
        "linux::state_tests::failed_global_exit_scan_does_not_confirm_an_empty_family",
        || {
            let mut supervisor = supervisor();
            supervisor.begin_drain();
            refuse_syscall(libc::SYS_waitid, libc::EIO)?;

            assert_eq!(supervisor.tick_family(), Err(ErrorCode::IdentityUnknown));
            assert!(matches!(supervisor.phase, Phase::Draining));
            assert!(supervisor.terminal_at.is_none());
            assert!(supervisor.writer.pending.is_empty());
            Ok(())
        },
    )
}

#[test]
fn failed_global_exit_scan_does_not_confirm_an_empty_family() -> TestResult {
    worker_failed_global_exit_scan_does_not_confirm_an_empty_family()
}

pub(in crate::linux) fn worker_an_unexpectedly_reaped_root_does_not_authorize_absence() -> TestResult {
    isolated(
        "linux::state_tests::an_unexpectedly_reaped_root_does_not_authorize_absence",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            guard.state.command(Command::Release {
                execution_id: guard.state.execution.clone(),
            });
            let root = guard
                .state
                .root
                .as_ref()
                .ok_or_else(|| io::Error::other("running root"))?;
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(root.pid, &mut status, 0) }, root.pid);

            assert_eq!(guard.state.tick_family(), Err(ErrorCode::IdentityUnknown));
            assert!(guard.state.root.as_ref().is_some_and(|root| !root.reaped));
            assert!(guard.state.terminal_at.is_none());
            assert!(matches!(guard.state.phase, Phase::Running));
            Ok(())
        },
    )
}

#[test]
fn an_unexpectedly_reaped_root_does_not_authorize_absence() -> TestResult {
    worker_an_unexpectedly_reaped_root_does_not_authorize_absence()
}

pub(in crate::linux) fn worker_an_empty_preinitialization_scan_does_not_emit_terminal_absence() -> TestResult {
    isolated(
        "linux::state_tests::an_empty_preinitialization_scan_does_not_emit_terminal_absence",
        || {
            let mut supervisor = supervisor();

            assert_eq!(supervisor.tick_family(), Ok(()));
            assert!(matches!(supervisor.phase, Phase::AwaitingInit));
            assert!(supervisor.terminal_at.is_none());
            assert!(supervisor.writer.pending.is_empty());
            Ok(())
        },
    )
}

#[test]
fn an_empty_preinitialization_scan_does_not_emit_terminal_absence() -> TestResult {
    worker_an_empty_preinitialization_scan_does_not_emit_terminal_absence()
}

pub(in crate::linux) fn worker_terminal_control_queue_failure_retains_the_absence_fact() -> TestResult {
    isolated(
        "linux::state_tests::terminal_control_queue_failure_retains_the_absence_fact",
        || {
            let mut supervisor = supervisor();
            supervisor.begin_drain();
            fill_control_queue(&mut supervisor)?;

            assert_eq!(supervisor.tick_family(), Ok(()));
            require(
                matches!(supervisor.phase, Phase::Draining),
                "unadmitted terminal must not be acknowledgeable",
            )?;
            let terminal_at = supervisor.terminal_at;
            assert!(terminal_at.is_some());
            assert!(!supervisor.acknowledged);
            let retained = supervisor.writer.pending.len();
            let delivered = queued_events(&mut supervisor.writer)?;
            assert_eq!(delivered.len(), retained);
            assert_eq!(supervisor.writer.control_bytes, 0);
            assert_eq!(supervisor.tick_family(), Ok(()));
            assert!(matches!(supervisor.phase, Phase::Absent));
            assert_eq!(supervisor.terminal_at, terminal_at);
            let events = queued_events(&mut supervisor.writer)?;
            assert!(matches!(events.as_slice(), [Event::FamilyAbsent { .. }]));
            Ok(())
        },
    )
}

#[test]
fn terminal_control_queue_failure_retains_the_absence_fact() -> TestResult {
    worker_terminal_control_queue_failure_retains_the_absence_fact()
}

pub(in crate::linux) fn worker_helper_identity_failure_retains_the_unreleased_root_for_cleanup() -> TestResult {
    isolated(
        "linux::state_tests::helper_identity_failure_retains_the_unreleased_root_for_cleanup",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            refuse_syscall(libc::SYS_getpid, libc::EPERM)?;

            guard.initialize()?;

            assert!(matches!(guard.state.phase, Phase::Draining));
            assert!(guard.state.canceled);
            assert!(!guard.state.released);
            assert!(guard.state.cleanup_started.is_some());
            assert!(guard.state.root.as_ref().is_some_and(|root| root.gate.is_none()));
            assert_eq!(guard.state.writer.pending.len(), 1);
            guard.confirm_cleanup()
        },
    )
}

#[test]
fn helper_identity_failure_retains_the_unreleased_root_for_cleanup() -> TestResult {
    worker_helper_identity_failure_retains_the_unreleased_root_for_cleanup()
}

pub(in crate::linux) fn worker_no_root_cannot_report_soft_signal_delivery() {
    let mut supervisor = supervisor();

    assert_eq!(supervisor.soft_signal(libc::SIGCONT), Ok(false));
    assert!(supervisor.root.is_none());
    assert!(supervisor.writer.pending.is_empty());
    assert!(!supervisor.released);
}

#[test]
fn no_root_cannot_report_soft_signal_delivery() {
    worker_no_root_cannot_report_soft_signal_delivery()
}

pub(in crate::linux) fn worker_a_reaped_root_never_uses_its_old_process_group_for_soft_signals() -> TestResult {
    isolated(
        "linux::state_tests::a_reaped_root_never_uses_its_old_process_group_for_soft_signals",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            guard.state.command(Command::Release {
                execution_id: guard.state.execution.clone(),
            });
            let root = guard
                .state
                .root
                .as_mut()
                .ok_or_else(|| io::Error::other("running root"))?;
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(root.pid, &mut status, 0) }, root.pid);
            root.status = RootStatus::from_raw(status);
            assert!(root.status.is_some());
            root.reaped = true;
            refuse_syscall(libc::SYS_kill, libc::EPERM)?;

            assert_eq!(guard.state.soft_signal(libc::SIGCONT), Ok(false));
            assert!(guard.state.root.as_ref().is_some_and(|root| root.reaped));
            assert!(guard.state.terminal_at.is_none());
            Ok(())
        },
    )
}

#[test]
fn a_reaped_root_never_uses_its_old_process_group_for_soft_signals() -> TestResult {
    worker_a_reaped_root_never_uses_its_old_process_group_for_soft_signals()
}

#[cfg(any(
    target_arch = "aarch64",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64"
))]
const POLL_SYSCALL: libc::c_long = libc::SYS_ppoll;
#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64"
)))]
const POLL_SYSCALL: libc::c_long = libc::SYS_poll;

pub(in crate::linux) fn worker_an_unready_descendant_pidfd_is_not_counted_as_reaped() -> TestResult {
    isolated(
        "linux::state_tests::an_unready_descendant_pidfd_is_not_counted_as_reaped",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let companion = guard.companion(true)?;
            observe_exit(companion.id() as i32)?;
            refuse_syscall(POLL_SYSCALL, 0)?;

            assert_eq!(guard.state.tick_family(), Ok(()));
            assert_eq!(guard.state.reaped, 0);
            assert!(guard.state.terminal_at.is_none());
            assert!(matches!(guard.state.phase, Phase::Held));
            observe_exit(companion.id() as i32)?;
            guard.confirm_cleanup()
        },
    )
}

#[test]
fn an_unready_descendant_pidfd_is_not_counted_as_reaped() -> TestResult {
    worker_an_unready_descendant_pidfd_is_not_counted_as_reaped()
}

pub(in crate::linux) fn worker_a_zero_result_reap_does_not_count_or_settle_the_descendant() -> TestResult {
    isolated(
        "linux::state_tests::a_zero_result_reap_does_not_count_or_settle_the_descendant",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let companion = guard.companion(true)?;
            observe_exit(companion.id() as i32)?;
            refuse_syscall(libc::SYS_wait4, 0)?;

            assert_eq!(guard.state.tick_family(), Ok(()));
            assert_eq!(guard.state.reaped, 0);
            assert!(guard.state.terminal_at.is_none());
            assert!(matches!(guard.state.phase, Phase::Held));
            observe_exit(companion.id() as i32)?;
            guard.confirm_cleanup()
        },
    )
}

#[test]
fn a_zero_result_reap_does_not_count_or_settle_the_descendant() -> TestResult {
    worker_a_zero_result_reap_does_not_count_or_settle_the_descendant()
}

fn defer_output_reads(stdout: RawFd, stderr: RawFd) -> TestResult {
    let mut filter = [
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 4,
            k: libc::SYS_read as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 16,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 1,
            jf: 0,
            k: stdout as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: stderr as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | libc::EAGAIN as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0
        || unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

pub(in crate::linux) fn worker_terminal_settlement_marks_unfinished_pipe_reads_as_truncated() -> TestResult {
    isolated(
        "linux::state_tests::terminal_settlement_marks_unfinished_pipe_reads_as_truncated",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            guard.state.command(Command::Release {
                execution_id: guard.state.execution.clone(),
            });
            let root = guard
                .state
                .root
                .as_mut()
                .ok_or_else(|| io::Error::other("running root"))?;
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(root.pid, &mut status, 0) }, root.pid);
            root.status = RootStatus::from_raw(status);
            assert!(
                root.status
                    .is_some_and(|status| status.code == Some(0) && status.signal.is_none())
            );
            root.reaped = true;
            let stdout = root
                .stdout
                .as_ref()
                .ok_or_else(|| io::Error::other("stdout pipe"))?
                .as_raw_fd();
            let stderr = root
                .stderr
                .as_ref()
                .ok_or_else(|| io::Error::other("stderr pipe"))?
                .as_raw_fd();
            guard.state.reaped = 1;
            defer_output_reads(stdout, stderr)?;

            assert_eq!(guard.state.tick_family(), Ok(()));
            assert!(matches!(guard.state.phase, Phase::Absent));
            assert!(guard.state.released);
            assert_eq!(guard.state.reaped, 1);
            assert!(guard.state.terminal_at.is_some());
            assert!(guard.state.writer.stdout_truncated && guard.state.writer.stderr_truncated);
            assert!(guard.state.root.as_ref().is_some_and(|root| {
                root.stdout.is_none()
                    && root.stderr.is_none()
                    && root
                        .status
                        .is_some_and(|status| status.code == Some(0) && status.signal.is_none())
            }));
            Ok(())
        },
    )
}

#[test]
fn terminal_settlement_marks_unfinished_pipe_reads_as_truncated() -> TestResult {
    worker_terminal_settlement_marks_unfinished_pipe_reads_as_truncated()
}

fn refuse_argument(syscall: libc::c_long, argument: i32, error: i32) -> TestResult {
    let mut filter = [
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 3,
            k: syscall as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 16,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: argument as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO | error as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0
        || unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn queued_events(writer: &mut Writer) -> Result<Vec<Event>, Box<dyn std::error::Error>> {
    let saved = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 10) };
    if saved < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let saved = unsafe { OwnedFd::from_raw_fd(saved) };
    let (read, write) = pipe()?;
    let receiver = std::thread::spawn(move || -> Result<Vec<u8>, io::Error> {
        let mut bytes = Vec::new();
        std::fs::File::from(read).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    if unsafe { libc::dup2(write.as_raw_fd(), libc::STDOUT_FILENO) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    writer.flush();
    let restored = unsafe { libc::dup2(saved.as_raw_fd(), libc::STDOUT_FILENO) };
    assert_eq!(restored, libc::STDOUT_FILENO);
    drop(write);
    let bytes = receiver
        .join()
        .map_err(|_| io::Error::other("control frame receiver panicked"))??;
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| crate::protocol::decode_event(line).map_err(Into::into))
        .collect()
}

pub(in crate::linux) fn worker_release_after_the_hold_deadline_never_opens_the_gate() -> TestResult {
    isolated(
        "linux::state_tests::release_after_the_hold_deadline_never_opens_the_gate",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            guard.state.release_deadline = Some(Instant::now() - Duration::from_millis(1));
            guard.state.command(Command::Release {
                execution_id: guard.state.execution.clone(),
            });
            require(!guard.state.released, "expired release opened the launch gate")?;
            assert!(guard.state.timed_out);
            assert!(matches!(guard.state.phase, Phase::Draining));
            assert!(guard.state.command_deadline.is_none());
            let events = queued_events(&mut guard.state.writer)?;
            assert!(!events.iter().any(|event| matches!(event, Event::Released { .. })));
            guard.confirm_cleanup()
        },
    )
}

#[test]
fn release_after_the_hold_deadline_never_opens_the_gate() -> TestResult {
    worker_release_after_the_hold_deadline_never_opens_the_gate()
}

pub(in crate::linux) fn worker_a_root_signal_refusal_still_signals_and_waits_other_owned_children() -> TestResult {
    isolated(
        "linux::state_tests::a_root_signal_refusal_still_signals_and_waits_other_owned_children",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let settled = guard.companion(true)?;
            let live = guard.companion(false)?;
            observe_exit(settled.id() as i32)?;
            guard.state.begin_drain();
            let root = guard.state.root.as_ref().ok_or_else(|| io::Error::other("held root"))?;
            observe_exit(root.pid)?;
            refuse_argument(libc::SYS_pidfd_send_signal, root.pidfd.as_raw_fd(), libc::EPERM)?;
            assert_eq!(guard.state.tick_family(), Err(ErrorCode::IdentityUnknown));
            require(
                guard.state.root.as_ref().is_some_and(|root| root.status.is_some()),
                "signal refusal skipped root exit observation",
            )?;
            observe_exit(live.id() as i32)?;
            assert!(guard.state.reaped >= 1);
            assert!(guard.state.terminal_at.is_none());
            Ok(())
        },
    )
}

#[test]
fn a_root_signal_refusal_still_signals_and_waits_other_owned_children() -> TestResult {
    worker_a_root_signal_refusal_still_signals_and_waits_other_owned_children()
}

pub(in crate::linux) fn worker_one_descendant_identity_failure_does_not_skip_later_owned_children() -> TestResult {
    isolated(
        "linux::state_tests::one_descendant_identity_failure_does_not_skip_later_owned_children",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let refused = guard.companion(true)?;
            let live = guard.companion(false)?;
            observe_exit(refused.id() as i32)?;
            guard.state.begin_drain();
            refuse_argument(libc::SYS_pidfd_open, refused.id() as i32, libc::EPERM)?;
            assert_eq!(guard.state.tick_family(), Err(ErrorCode::IdentityUnknown));
            observe_exit(live.id() as i32)?;
            assert!(matches!(guard.state.phase, Phase::Draining));
            assert!(guard.state.terminal_at.is_none());
            Ok(())
        },
    )
}

#[test]
fn one_descendant_identity_failure_does_not_skip_later_owned_children() -> TestResult {
    worker_one_descendant_identity_failure_does_not_skip_later_owned_children()
}

pub(in crate::linux) fn worker_a_permanent_setup_read_failure_reports_unknown_exec_but_exact_family_absence()
-> TestResult {
    isolated(
        "linux::state_tests::a_permanent_setup_read_failure_reports_unknown_exec_but_exact_family_absence",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            let mut command = initialization(&guard.state)?;
            if let Command::Init { argv, .. } = &mut command {
                *argv = crate::Arguments::new(vec!["/missing/sandbox-executable".into()])?;
            }
            guard.state.command(command);
            require(
                guard.state.root.is_some(),
                "missing executable was not held for release",
            )?;
            guard.state.command(Command::Release {
                execution_id: guard.state.execution.clone(),
            });
            let root = guard
                .state
                .root
                .as_ref()
                .ok_or_else(|| io::Error::other("running root"))?;
            observe_exit(root.pid)?;
            refuse_argument(libc::SYS_read, root.setup.as_raw_fd(), libc::EIO)?;
            guard.state.observe_exec();
            assert_eq!(guard.state.tick_family(), Ok(()));
            assert!(matches!(guard.state.phase, Phase::Absent));
            let events = queued_events(&mut guard.state.writer)?;
            require(
                events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            Event::Error {
                                code: ErrorCode::IoFailure,
                                uncertain: crate::Certainty::Uncertain,
                                ..
                            }
                        )
                    })
                    .count()
                    == 1,
                "permanent setup read failure omitted unknown exec outcome",
            )?;
            require(
                matches!(
                    events.last(),
                    Some(Event::FamilyAbsent {
                        root_exit_code: Some(127),
                        ..
                    })
                ),
                "exec uncertainty changed exact family/root exit settlement",
            )?;
            Ok(())
        },
    )
}

#[test]
fn a_permanent_setup_read_failure_reports_unknown_exec_but_exact_family_absence() -> TestResult {
    worker_a_permanent_setup_read_failure_reports_unknown_exec_but_exact_family_absence()
}

fn require(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}

pub(in crate::linux) fn worker_interrupted_setup_reads_retry_before_observing_exec_failure() -> TestResult {
    isolated(
        "linux::state_tests::interrupted_setup_reads_retry_before_observing_exec_failure",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let (read, write) = pipe()?;
            guard
                .state
                .root
                .as_mut()
                .ok_or_else(|| io::Error::other("held root"))?
                .setup = read;
            let descriptor = guard
                .state
                .root
                .as_ref()
                .ok_or_else(|| io::Error::other("held root"))?
                .setup
                .as_raw_fd();
            let observer = read_interruption::interrupt_then_read_packet(descriptor, write)?;
            guard.state.observe_exec();
            let observed = observer
                .join()
                .map_err(|_| io::Error::other("setup read observer panicked"))??;
            require(
                observed == 2,
                "setup read did not receive interruption followed by packet continuation",
            )?;
            let events = queued_events(&mut guard.state.writer)?;
            require(
                events.iter().any(|event| {
                    matches!(
                        event,
                        Event::Error {
                            code: ErrorCode::ExecFailed,
                            uncertain: crate::Certainty::Certain,
                            ..
                        }
                    )
                }),
                "interrupted setup read did not retry the available failure packet",
            )?;
            guard.confirm_cleanup()
        },
    )
}

#[test]
fn interrupted_setup_reads_retry_before_observing_exec_failure() -> TestResult {
    worker_interrupted_setup_reads_retry_before_observing_exec_failure()
}

pub(in crate::linux) fn worker_a_would_block_setup_read_keeps_the_outcome_pending_until_a_later_packet() -> TestResult {
    isolated(
        "linux::state_tests::a_would_block_setup_read_keeps_the_outcome_pending_until_a_later_packet",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let (read, write) = pipe()?;
            nonblocking(read.as_raw_fd())?;
            guard
                .state
                .root
                .as_mut()
                .ok_or_else(|| io::Error::other("held root"))?
                .setup = read;
            let queued = guard.state.writer.pending.len();
            guard.state.observe_exec();
            require(
                guard.state.writer.pending.len() == queued,
                "would-block setup read reported failure",
            )?;
            std::io::Write::write_all(&mut std::fs::File::from(write), &[1, 2, 0, 0, 0])?;
            guard.state.observe_exec();
            guard.state.observe_exec();
            let events = queued_events(&mut guard.state.writer)?;
            require(
                events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            Event::Error {
                                code: ErrorCode::ExecFailed,
                                ..
                            }
                        )
                    })
                    .count()
                    == 1,
                "deferred setup failure was lost or duplicated",
            )?;
            guard.confirm_cleanup()
        },
    )
}

#[test]
fn a_would_block_setup_read_keeps_the_outcome_pending_until_a_later_packet() -> TestResult {
    worker_a_would_block_setup_read_keeps_the_outcome_pending_until_a_later_packet()
}

pub(in crate::linux) fn worker_an_unadmitted_terminal_event_expires_at_the_original_ack_deadline() -> TestResult {
    isolated(
        "linux::state_tests::an_unadmitted_terminal_event_expires_at_the_original_ack_deadline",
        || {
            let mut supervisor = supervisor();
            supervisor.begin_drain();
            fill_control_queue(&mut supervisor)?;
            require(
                supervisor.tick_family() == Ok(()),
                "family absence could not be retained",
            )?;
            require(
                matches!(supervisor.phase, Phase::Draining),
                "unadmitted terminal event became acknowledgeable",
            )?;
            supervisor.terminal_at = Some(Instant::now() - ACK_TIMEOUT);
            refuse_argument(libc::SYS_write, libc::STDOUT_FILENO, libc::EAGAIN)?;
            let started = Instant::now();
            let result = supervisor.run_loop();
            require(
                result == 74,
                "undeliverable terminal event did not report I/O retirement failure",
            )?;
            require(
                started.elapsed() < Duration::from_secs(1),
                "terminal admission restarted the retirement deadline",
            )?;
            require(
                supervisor.pending_terminal.is_some(),
                "pending terminal receipt was dropped",
            )?;
            require(
                !supervisor.acknowledged,
                "undelivered terminal receipt was acknowledged",
            )?;
            require(
                supervisor.writer.control_bytes <= QUEUED_CONTROL_LIMIT,
                "control queue budget was exceeded",
            )?;
            Ok(())
        },
    )
}

#[test]
fn an_unadmitted_terminal_event_expires_at_the_original_ack_deadline() -> TestResult {
    worker_an_unadmitted_terminal_event_expires_at_the_original_ack_deadline()
}

pub(in crate::linux) fn worker_a_child_setup_error_is_preserved_at_the_supervisor_boundary() -> TestResult {
    isolated(
        "linux::state_tests::a_child_setup_error_is_preserved_at_the_supervisor_boundary",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            let mut command = initialization(&guard.state)?;
            if let Command::Init { cwd, .. } = &mut command {
                *cwd = Some(crate::WorkingDirectory::new("/missing/sandbox-directory".into())?);
            }
            guard.state.command(command);
            let children = direct_children(MAX_CHILDREN)?;
            for child in &children.children {
                guard.owned.push((*child, pidfd(*child)?));
            }
            require(
                !children.has_more && guard.owned.len() == 1,
                "failed setup child was not retained for exact cleanup",
            )?;
            require(
                guard.state.root.is_none(),
                "failed child setup retained a usable workload",
            )?;
            require(
                matches!(guard.state.phase, Phase::Draining),
                "failed child setup did not retain cleanup",
            )?;
            let events = queued_events(&mut guard.state.writer)?;
            require(
                matches!(
                    events.as_slice(),
                    [Event::Error {
                        code: ErrorCode::SetupFailed,
                        uncertain: crate::Certainty::Uncertain,
                        ..
                    }]
                ),
                "typed child setup failure lost its wire classification",
            )?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while guard.state.phase != Phase::Absent {
                guard.state.tick_family()?;
                require(
                    Instant::now() < deadline,
                    "failed setup child cleanup exceeded its deadline",
                )?;
                std::thread::sleep(Duration::from_millis(1));
            }
            require(
                guard.state.reaped == 1,
                "failed setup child was not counted as exactly reaped",
            )?;
            for (child, descriptor) in &guard.owned {
                require(
                    readable(descriptor)?,
                    "failed setup child remained alive after terminal settlement",
                )?;
                let mut status = 0;
                require(
                    unsafe { libc::waitpid(*child, &mut status, libc::WNOHANG) } == -1 && errno() == libc::ECHILD,
                    "failed setup child remained waitable after terminal settlement",
                )?;
            }
            guard.owned.clear();
            Ok(())
        },
    )
}

#[test]
fn a_child_setup_error_is_preserved_at_the_supervisor_boundary() -> TestResult {
    worker_a_child_setup_error_is_preserved_at_the_supervisor_boundary()
}

pub(in crate::linux) fn worker_overflow_signal_failure_still_observes_the_owned_root_exit() -> TestResult {
    isolated(
        "linux::state_tests::overflow_signal_failure_still_observes_the_owned_root_exit",
        || {
            probe()?;
            let mut guard = StateGuard::new();
            guard.initialize()?;
            let _companion = guard.companion(false)?;
            guard.state.family_limit = 1;
            let root = guard.state.root.as_mut().ok_or_else(|| io::Error::other("held root"))?;
            root.gate.take();
            observe_exit(root.pid)?;
            refuse_argument(libc::SYS_pidfd_send_signal, root.pidfd.as_raw_fd(), libc::EPERM)?;
            require(
                guard.state.tick_family() == Err(ErrorCode::IdentityUnknown),
                "overflow signal refusal lost its uncertainty",
            )?;
            require(
                guard
                    .state
                    .root
                    .as_ref()
                    .is_some_and(|root| root.status.is_some() && !root.reaped),
                "overflow signal failure skipped root observation or prematurely retired custody",
            )?;
            require(
                guard.state.process_limit_notice,
                "overflow lost the process limit receipt",
            )?;
            require(
                guard.state.terminal_at.is_none(),
                "overflow with unresolved custody reported exact absence",
            )?;
            Ok(())
        },
    )
}

#[test]
fn overflow_signal_failure_still_observes_the_owned_root_exit() -> TestResult {
    worker_overflow_signal_failure_still_observes_the_owned_root_exit()
}

fn deferred_setup_read_settlement(error: i32) -> TestResult {
    probe()?;
    let mut guard = StateGuard::new();
    let mut command = initialization(&guard.state)?;
    if let Command::Init { argv, .. } = &mut command {
        *argv = crate::Arguments::new(vec!["/missing/sandbox-executable".into()])?;
    }
    guard.state.command(command);
    require(
        guard.state.root.is_some(),
        "missing executable was not held for release",
    )?;
    guard.state.command(Command::Release {
        execution_id: guard.state.execution.clone(),
    });
    let root = guard
        .state
        .root
        .as_ref()
        .ok_or_else(|| io::Error::other("running root"))?;
    observe_exit(root.pid)?;
    refuse_argument(libc::SYS_read, root.setup.as_raw_fd(), error)?;
    let queued = guard.state.writer.pending.len();
    guard.state.observe_exec();
    require(
        guard.state.writer.pending.len() == queued,
        "temporary setup read refusal was not deferred while family settlement was pending",
    )?;
    guard.state.tick_family()?;
    require(
        matches!(guard.state.phase, Phase::Absent),
        "unknown exec outcome blocked exact family settlement",
    )?;
    let events = queued_events(&mut guard.state.writer)?;
    require(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::Error {
                        code: ErrorCode::IoFailure,
                        uncertain: crate::Certainty::Uncertain,
                        ..
                    }
                )
            })
            .count()
            == 1,
        "final deferred setup read lost the unknown exec outcome",
    )?;
    require(
        !events.iter().any(|event| {
            matches!(
                event,
                Event::Error {
                    code: ErrorCode::ExecFailed,
                    ..
                }
            )
        }),
        "unobserved setup outcome fabricated exec failure",
    )?;
    require(
        matches!(
            events.last(),
            Some(Event::FamilyAbsent {
                root_exit_code: Some(127),
                reaped_count: 1,
                ..
            })
        ),
        "unknown exec outcome changed exact root/family settlement",
    )?;
    Ok(())
}

pub(in crate::linux) fn worker_final_would_block_setup_read_reports_unknown_exec_with_exact_absence() -> TestResult {
    isolated(
        "linux::state_tests::final_would_block_setup_read_reports_unknown_exec_with_exact_absence",
        || deferred_setup_read_settlement(libc::EAGAIN),
    )
}

#[test]
fn final_would_block_setup_read_reports_unknown_exec_with_exact_absence() -> TestResult {
    worker_final_would_block_setup_read_reports_unknown_exec_with_exact_absence()
}

pub(in crate::linux) fn worker_final_interrupted_setup_read_reports_unknown_exec_with_exact_absence() -> TestResult {
    isolated(
        "linux::state_tests::final_interrupted_setup_read_reports_unknown_exec_with_exact_absence",
        || deferred_setup_read_settlement(libc::EINTR),
    )
}

#[test]
fn final_interrupted_setup_read_reports_unknown_exec_with_exact_absence() -> TestResult {
    worker_final_interrupted_setup_read_reports_unknown_exec_with_exact_absence()
}

pub(in crate::linux) fn dispatch_worker(name: &str) -> Option<crate::linux::tests::TestResult> {
    match name {
        "linux::state_tests::a_control_command_cannot_be_prepared_as_a_workload" => Some({
            worker_a_control_command_cannot_be_prepared_as_a_workload();
            Ok(())
        }),
        "linux::state_tests::failed_error_delivery_closes_control_and_retains_drain" => {
            Some(worker_failed_error_delivery_closes_control_and_retains_drain().map_err(Into::into))
        }
        "linux::state_tests::failed_signal_receipt_still_initiates_cleanup" => {
            Some(worker_failed_signal_receipt_still_initiates_cleanup().map_err(Into::into))
        }
        "linux::state_tests::confirmed_absence_remains_terminal_when_cleanup_is_repeated" => Some({
            worker_confirmed_absence_remains_terminal_when_cleanup_is_repeated();
            Ok(())
        }),
        "linux::state_tests::foreign_acknowledgment_cannot_retire_the_anchor" => {
            Some(worker_foreign_acknowledgment_cannot_retire_the_anchor().map_err(Into::into))
        }
        "linux::state_tests::release_before_initialization_cannot_advance_execution" => Some({
            worker_release_before_initialization_cannot_advance_execution();
            Ok(())
        }),
        "linux::state_tests::missing_held_gate_cannot_be_reported_as_released" => Some({
            worker_missing_held_gate_cannot_be_reported_as_released();
            Ok(())
        }),
        "linux::state_tests::process_limit_notice_is_once_and_cleanup_clock_does_not_restart" => Some({
            worker_process_limit_notice_is_once_and_cleanup_clock_does_not_restart();
            Ok(())
        }),
        "linux::state_tests::replay_of_unsettled_or_failed_signal_keeps_original_outcome" => {
            Some(worker_replay_of_unsettled_or_failed_signal_keeps_original_outcome().map_err(Into::into))
        }
        "linux::state_tests::conflicting_or_older_signal_replay_cannot_replace_the_original" => {
            Some(worker_conflicting_or_older_signal_replay_cannot_replace_the_original().map_err(Into::into))
        }
        "linux::state_tests::signal_after_absence_reports_no_delivery_without_reopening_cleanup" => {
            Some(worker_signal_after_absence_reports_no_delivery_without_reopening_cleanup().map_err(Into::into))
        }
        "linux::state_tests::unavailable_current_directory_cannot_create_a_held_root" => {
            Some(worker_unavailable_current_directory_cannot_create_a_held_root())
        }
        "linux::state_tests::an_undeliverable_ready_receipt_keeps_the_root_held_and_drains" => {
            Some(worker_an_undeliverable_ready_receipt_keeps_the_root_held_and_drains())
        }
        "linux::state_tests::process_group_signal_refusal_does_not_claim_delivery" => {
            Some(worker_process_group_signal_refusal_does_not_claim_delivery())
        }
        "linux::state_tests::incomplete_family_signal_scan_is_not_reported_as_complete" => {
            Some(worker_incomplete_family_signal_scan_is_not_reported_as_complete())
        }
        "linux::state_tests::stdin_read_failure_closes_input_and_begins_cleanup" => {
            Some(worker_stdin_read_failure_closes_input_and_begins_cleanup())
        }
        "linux::state_tests::failed_output_reads_mark_each_stream_truncated" => {
            Some(worker_failed_output_reads_mark_each_stream_truncated())
        }
        "linux::state_tests::failed_exit_observation_cannot_publish_family_absence" => {
            Some(worker_failed_exit_observation_cannot_publish_family_absence())
        }
        "linux::state_tests::a_descendant_reap_failure_does_not_increment_settlement" => {
            Some(worker_a_descendant_reap_failure_does_not_increment_settlement())
        }
        "linux::state_tests::a_root_reap_failure_does_not_confirm_cleanup" => {
            Some(worker_a_root_reap_failure_does_not_confirm_cleanup())
        }
        "linux::state_tests::failed_global_exit_scan_does_not_confirm_an_empty_family" => {
            Some(worker_failed_global_exit_scan_does_not_confirm_an_empty_family())
        }
        "linux::state_tests::an_unexpectedly_reaped_root_does_not_authorize_absence" => {
            Some(worker_an_unexpectedly_reaped_root_does_not_authorize_absence())
        }
        "linux::state_tests::an_empty_preinitialization_scan_does_not_emit_terminal_absence" => {
            Some(worker_an_empty_preinitialization_scan_does_not_emit_terminal_absence())
        }
        "linux::state_tests::terminal_control_queue_failure_retains_the_absence_fact" => {
            Some(worker_terminal_control_queue_failure_retains_the_absence_fact())
        }
        "linux::state_tests::helper_identity_failure_retains_the_unreleased_root_for_cleanup" => {
            Some(worker_helper_identity_failure_retains_the_unreleased_root_for_cleanup())
        }
        "linux::state_tests::no_root_cannot_report_soft_signal_delivery" => Some({
            worker_no_root_cannot_report_soft_signal_delivery();
            Ok(())
        }),
        "linux::state_tests::a_reaped_root_never_uses_its_old_process_group_for_soft_signals" => {
            Some(worker_a_reaped_root_never_uses_its_old_process_group_for_soft_signals())
        }
        "linux::state_tests::an_unready_descendant_pidfd_is_not_counted_as_reaped" => {
            Some(worker_an_unready_descendant_pidfd_is_not_counted_as_reaped())
        }
        "linux::state_tests::a_zero_result_reap_does_not_count_or_settle_the_descendant" => {
            Some(worker_a_zero_result_reap_does_not_count_or_settle_the_descendant())
        }
        "linux::state_tests::terminal_settlement_marks_unfinished_pipe_reads_as_truncated" => {
            Some(worker_terminal_settlement_marks_unfinished_pipe_reads_as_truncated())
        }
        "linux::state_tests::release_after_the_hold_deadline_never_opens_the_gate" => {
            Some(worker_release_after_the_hold_deadline_never_opens_the_gate())
        }
        "linux::state_tests::a_root_signal_refusal_still_signals_and_waits_other_owned_children" => {
            Some(worker_a_root_signal_refusal_still_signals_and_waits_other_owned_children())
        }
        "linux::state_tests::one_descendant_identity_failure_does_not_skip_later_owned_children" => {
            Some(worker_one_descendant_identity_failure_does_not_skip_later_owned_children())
        }
        "linux::state_tests::a_permanent_setup_read_failure_reports_unknown_exec_but_exact_family_absence" => {
            Some(worker_a_permanent_setup_read_failure_reports_unknown_exec_but_exact_family_absence())
        }
        "linux::state_tests::interrupted_setup_reads_retry_before_observing_exec_failure" => {
            Some(worker_interrupted_setup_reads_retry_before_observing_exec_failure())
        }
        "linux::state_tests::a_would_block_setup_read_keeps_the_outcome_pending_until_a_later_packet" => {
            Some(worker_a_would_block_setup_read_keeps_the_outcome_pending_until_a_later_packet())
        }
        "linux::state_tests::an_unadmitted_terminal_event_expires_at_the_original_ack_deadline" => {
            Some(worker_an_unadmitted_terminal_event_expires_at_the_original_ack_deadline())
        }
        "linux::state_tests::a_child_setup_error_is_preserved_at_the_supervisor_boundary" => {
            Some(worker_a_child_setup_error_is_preserved_at_the_supervisor_boundary())
        }
        "linux::state_tests::overflow_signal_failure_still_observes_the_owned_root_exit" => {
            Some(worker_overflow_signal_failure_still_observes_the_owned_root_exit())
        }
        "linux::state_tests::final_would_block_setup_read_reports_unknown_exec_with_exact_absence" => {
            Some(worker_final_would_block_setup_read_reports_unknown_exec_with_exact_absence())
        }
        "linux::state_tests::final_interrupted_setup_read_reports_unknown_exec_with_exact_absence" => {
            Some(worker_final_interrupted_setup_read_reports_unknown_exec_with_exact_absence())
        }
        _ => None,
    }
}
