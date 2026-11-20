use super::syscall_notifications::{Action, PendingListener, Policy, Syscall};
use super::*;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::process::Output;

struct CapturedChild(Option<Child>);

impl Drop for CapturedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn capture(command: &mut Process) -> io::Result<Output> {
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut guard = CapturedChild(Some(child));
    let until = Instant::now() + WAIT;
    loop {
        let child = guard
            .0
            .as_mut()
            .ok_or_else(|| io::Error::other("missing captured child"))?;
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= until {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    guard
        .0
        .take()
        .ok_or_else(|| io::Error::other("missing captured child"))?
        .wait_with_output()
}

#[test]
fn malformed_fixture_invocations_refuse_without_user_effects() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("effect");
    for arguments in [
        Vec::<OsString>::new(),
        vec!["effect".into()],
        vec!["unknown-mode".into(), marker.clone().into_os_string()],
    ] {
        let mut command = Process::new(env!("CARGO_BIN_EXE_supervisor-test-fixture"));
        command.args(arguments).env_clear().envs(coverage_environment());
        let output = capture(&mut command)?;
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert!(!marker.exists());
    }
    Ok(())
}

#[test]
fn malformed_and_oversized_inherited_environments_refuse_before_hello() -> Result<(), Box<dyn std::error::Error>> {
    for (key, value) in [
        (OsString::from_vec(vec![0xff]), "value".into()),
        ("INVALID_VALUE".into(), OsString::from_vec(vec![0xff])),
        (
            "OVERSIZED_VALUE".into(),
            "x".repeat(sandbox_command_supervisor::protocol::MAX_ENV_BYTES).into(),
        ),
    ] {
        let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
        command
            .args(["--execution-id", "contract"])
            .env_clear()
            .envs(coverage_environment())
            .env(key, value);
        let output = capture(&mut command)?;
        assert_eq!(output.status.code(), Some(64));
        let mut frames = output.stdout.as_slice();
        assert!(matches!(
            read_event(&mut frames)?,
            Some(Event::Error {
                code: ErrorCode::InvalidCommand,
                uncertain: Certainty::Certain,
                ..
            })
        ));
        assert!(read_event(&mut frames)?.is_none());
        assert!(output.stderr.is_empty());
    }
    Ok(())
}

#[test]
fn missing_startup_kernel_prerequisite_refuses_before_hello() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
    command
        .args(["--execution-id", "contract"])
        .env_clear()
        .envs(coverage_environment());
    deny_syscall(&mut command, libc::SYS_prctl);
    let output = capture(&mut command)?;
    assert_eq!(output.status.code(), Some(78));
    let mut frames = output.stdout.as_slice();
    assert!(matches!(
        read_event(&mut frames)?,
        Some(Event::Error {
            code: ErrorCode::KernelPrerequisite,
            uncertain: Certainty::Certain,
            ..
        })
    ));
    assert!(read_event(&mut frames)?.is_none());
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn escaped_middle_session_refusal_preserves_root_status_and_owned_absence() -> Result<(), Box<dyn std::error::Error>> {
    refused_middle(Syscall::Session, Policy::reject_after(1, libc::EPERM), 1, libc::EPERM)
}

#[test]
fn escaped_leaf_fork_refusal_preserves_root_status_and_owned_absence() -> Result<(), Box<dyn std::error::Error>> {
    refused_middle(Syscall::Fork, Policy::reject_after(2, libc::EAGAIN), 2, libc::EAGAIN)
}

fn refused_middle(
    syscall: Syscall,
    policy: Policy,
    allowed: usize,
    errno: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("escaped");
    let mut sentinel = Sentinel::start();
    let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
    let pending = PendingListener::install(&mut command, syscall, policy)?;
    let mut helper = Harness::start_command(command);
    let listener = pending.start()?;
    let identity = helper.initialize("escape-root-exits", &marker, None, None);
    let root = pin(identity);
    sentinel.assert_alive();
    helper.release();
    assert!(matches!(helper.absent(), Event::FamilyAbsent {
        released: ReleaseState::Released, root_exit_code: Some(7), root_signal: None,
        timed_out: TimeoutState::WithinDeadline, canceled: Cancellation::Active,
        stdout_truncated: Truncation::Complete, stderr_truncated: Truncation::Complete,
        reaped_count, ..
    } if reaped_count >= 1));
    assert_dead(&root);
    assert!(!marker.exists());
    sentinel.assert_alive();
    helper.acknowledge();
    let report = listener.finish()?;
    assert_eq!(report.continued, allowed);
    assert_eq!(report.refused, 1);
    assert_eq!(report.first.len(), allowed + 1);
    if allowed == 2 {
        assert_eq!(report.first[0].tid, helper.process.id());
        assert_eq!(report.first[1].tid, identity.pid);
    } else {
        assert_eq!(report.first[0].tid, identity.pid);
    }
    let denied = &report.first[allowed];
    assert_ne!(denied.tid, helper.process.id());
    assert_ne!(denied.tid, identity.pid);
    assert!(matches!(denied.action, Action::Refuse(actual) if actual == errno));
    let middle = denied
        .process
        .as_ref()
        .ok_or_else(|| io::Error::other("missing denied middle identity"))?;
    assert_dead(middle);
    sentinel.assert_alive();
    Ok(())
}

#[test]
fn fork_refusal_during_full_natural_churn_does_not_lose_family_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("churn");
    let mut sentinel = Sentinel::start();
    let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
    let pending = PendingListener::install(&mut command, Syscall::Fork, Policy::reject_once_after(1, libc::EAGAIN))?;
    let mut helper = Harness::start_command(command);
    let listener = pending.start()?;
    let identity = helper.initialize("churn", &marker, None, None);
    let root = pin(identity);
    let released_at = Instant::now();
    helper.release();
    let actual = recorded(&marker);
    assert_eq!(actual.pid, identity.pid);
    assert_eq!(actual.start_time, identity.start_time);
    sentinel.assert_alive();
    let until = released_at + Duration::from_secs(20);
    loop {
        let event = helper
            .events
            .recv_timeout(until.saturating_duration_since(Instant::now()))?;
        match event {
            Event::RootExited {
                exit_code: Some(0),
                signal: None,
                ..
            } => {}
            Event::FamilyAbsent {
                released: ReleaseState::Released,
                root_exit_code: Some(0),
                root_signal: None,
                timed_out: TimeoutState::WithinDeadline,
                canceled: Cancellation::Active,
                stdout_truncated: Truncation::Complete,
                stderr_truncated: Truncation::Complete,
                reaped_count,
                ..
            } if reaped_count >= 1 => break,
            _ => return Err(io::Error::other("natural churn did not retain successful owned absence").into()),
        }
    }
    assert!(released_at.elapsed() >= Duration::from_secs(10));
    assert_dead(&root);
    sentinel.assert_alive();
    helper.acknowledge();
    let report = listener.finish()?;
    assert_eq!(report.refused, 1);
    assert!(report.continued > 1);
    assert_eq!(report.first.len(), 3);
    assert_eq!(report.first[0].tid, helper.process.id());
    assert!(matches!(report.first[0].action, Action::Continue));
    assert!(matches!(report.first[1].action, Action::Refuse(libc::EAGAIN)));
    assert!(matches!(report.first[2].action, Action::Continue));
    sentinel.assert_alive();
    Ok(())
}
