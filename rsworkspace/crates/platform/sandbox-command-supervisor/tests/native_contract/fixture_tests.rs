use super::syscall_notifications::{Action, PendingListener, Policy, Syscall};
use super::*;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::process::Output;

struct CapturedChild {
    child: Option<Child>,
    custody: FamilyCustody,
}

impl Drop for CapturedChild {
    fn drop(&mut self) {
        let stopped = self
            .child
            .as_mut()
            .is_none_or(|child| stop_helper(child, Duration::ZERO));
        let settled = self.custody.settle();
        assert!(
            stopped && settled || std::thread::panicking(),
            "captured child custody must settle"
        );
    }
}

fn drain_capture(pipe: &mut impl std::io::Read, bytes: &mut Vec<u8>) -> io::Result<bool> {
    let mut chunk = [0_u8; 8192];
    for _ in 0..16 {
        match pipe.read(&mut chunk) {
            Ok(0) => return Ok(true),
            Ok(size) => {
                if bytes.len().saturating_add(size) > 1024 * 1024 {
                    return Err(io::Error::other("captured output exceeds its bound"));
                }
                bytes.extend_from_slice(&chunk[..size]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub(super) fn capture(command: &mut Process) -> io::Result<Output> {
    let custody = FamilyCustody::enter();
    let child = command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    capture_child(child, Instant::now() + WAIT, custody)
}

fn capture_child(child: Child, until: Instant, custody: FamilyCustody) -> io::Result<Output> {
    let mut guard = CapturedChild {
        child: Some(child),
        custody,
    };
    let child = guard
        .child
        .as_mut()
        .ok_or_else(|| io::Error::other("missing captured child"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing captured stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing captured stderr"))?;
    for descriptor in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut status = None;
    loop {
        if Instant::now() >= until {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let stdout_eof = drain_capture(&mut stdout, &mut output)?;
        let stderr_eof = drain_capture(&mut stderr, &mut errors)?;
        if stdout_eof && stderr_eof && status.is_none() {
            status = guard
                .child
                .as_mut()
                .ok_or_else(|| io::Error::other("missing captured child"))?
                .try_wait()?;
        }
        if stdout_eof
            && stderr_eof
            && let Some(status) = status
        {
            if Instant::now() >= until {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            guard.child.take();
            return Ok(Output {
                status,
                stdout: output,
                stderr: errors,
            });
        }
        if Instant::now() >= until {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn malformed_fixture_invocations_refuse_without_user_effects() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
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
    let _custody = TestCustody::enter();
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
    let _custody = TestCustody::enter();
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
    let _custody = TestCustody::enter();
    refused_middle(Syscall::Session, Policy::reject_after(1, libc::EPERM), 1, libc::EPERM)
}

#[test]
fn escaped_leaf_fork_refusal_preserves_root_status_and_owned_absence() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
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
    assert_exited(&root);
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
    assert_exited(middle);
    sentinel.assert_alive();
    Ok(())
}

#[test]
fn fork_refusal_during_full_natural_churn_does_not_lose_family_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
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
    assert_exited(&root);
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

#[test]
fn capture_deadline_includes_descendants_holding_output_pipes() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let mut command = Process::new("/bin/sh");
    command.args(["-c", &format!("sleep {} & exit 0", WAIT.as_secs() + 1)]);
    let result = capture(&mut command);
    assert!(
        result.is_err_and(|error| error.kind() == io::ErrorKind::TimedOut),
        "exited direct child must not remove the output EOF deadline"
    );
    Ok(())
}

#[test]
fn capture_refuses_completion_after_original_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let custody = FamilyCustody::enter();
    let mut child = Process::new("/bin/true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.wait()?;
    let result = capture_child(child, Instant::now() - Duration::from_millis(1), custody);
    assert!(
        result.is_err_and(|error| error.kind() == io::ErrorKind::TimedOut),
        "completed pipes and status cannot erase an expired capture deadline"
    );
    Ok(())
}
