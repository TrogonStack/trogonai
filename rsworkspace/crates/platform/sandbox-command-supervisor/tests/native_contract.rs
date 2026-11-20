#![cfg(target_os = "linux")]

#[cfg(coverage)]
use trogon_std::env::ReadEnv;

use sandbox_command_supervisor::protocol::{
    Cancellation, Certainty, Delivery, PlatformSupport, ReleaseState, TimeoutState, Truncation,
};

use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use sandbox_command_supervisor::protocol::{
    CommandWire as Command, ErrorCode, Event, ExecutionId, Identity, Phase, Stream, VERSION, read_event, write_command,
};

const WAIT: Duration = Duration::from_secs(8);

struct Harness {
    process: Child,
    input: Option<ChildStdin>,
    events: Receiver<Event>,
    execution: ExecutionId,
    output: Vec<u8>,
    errors: Vec<u8>,
    next_signal: u64,
}

impl Harness {
    fn start() -> Self {
        Self::start_binary(env!("CARGO_BIN_EXE_sandbox-command-supervisor"))
    }

    fn start_binary(binary: &str) -> Self {
        Self::start_command(Process::new(binary))
    }

    #[expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn start_command(mut command: Process) -> Self {
        let execution = ExecutionId::new("contract").unwrap();
        let mut process = command
            .args(["--execution-id", execution.as_str()])
            .env_clear()
            .envs(coverage_environment())
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start native helper");
        let output = process.stdout.take().unwrap();
        let (send, events) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Ok(Some(event)) = read_event(&mut reader) {
                if send.send(event).is_err() {
                    break;
                }
            }
        });
        let input = process.stdin.take();
        let mut harness = Self {
            process,
            input,
            events,
            execution,
            output: Vec::new(),
            errors: Vec::new(),
            next_signal: 1,
        };
        assert!(matches!(harness.next(), Event::Hello { version: VERSION, .. }));
        harness
    }

    #[expect(
        clippy::expect_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn send(&mut self, command: Command) {
        write_command(self.input.as_mut().expect("open host control"), &command).expect("write private command");
    }

    #[expect(
        clippy::expect_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn next(&mut self) -> Event {
        loop {
            let event = self.events.recv_timeout(WAIT).expect("bounded helper event");
            match event {
                Event::Output {
                    stream: Stream::Stdout,
                    data,
                    ..
                } => self.output.extend(data),
                Event::Output {
                    stream: Stream::Stderr,
                    data,
                    ..
                } => self.errors.extend(data),
                event => return event,
            }
        }
    }

    #[expect(
        clippy::panic,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn initialize(&mut self, mode: &str, path: &Path, timeout_ms: Option<u64>, release_ms: Option<u64>) -> Identity {
        self.send(Command::Init {
            version: VERSION,
            execution_id: self.execution.clone(),
            argv: vec![
                env!("CARGO_BIN_EXE_supervisor-test-fixture").into(),
                mode.into(),
                path.to_string_lossy().into_owned(),
            ],
            env: workload_environment(BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())])),
            cwd: None,
            timeout_ms,
            release_timeout_ms: release_ms,
        });
        match self.next() {
            Event::Ready {
                helper,
                root,
                process_group,
                session,
                ..
            } => {
                assert_eq!(helper.pid, self.process.id());
                assert!(helper.start_time > 0 && root.start_time > 0);
                assert_eq!(process_group, root.pid as i32);
                assert_eq!(session, root.pid as i32);
                root
            }
            _ => panic!("helper must hold a pinned root"),
        }
    }

    fn release(&mut self) {
        self.send(Command::Release {
            execution_id: self.execution.clone(),
        });
        assert!(matches!(self.next(), Event::Released { .. }));
    }

    #[expect(
        clippy::expect_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn signal(&mut self, signal: i32) {
        let request_id = self.next_signal;
        self.next_signal = request_id.checked_add(1).expect("test request sequence");
        self.send(Command::Signal {
            execution_id: self.execution.clone(),
            request_id,
            signal,
        });
        assert!(matches!(
            self.next(),
            Event::SignalResult { request_id: observed, .. } if observed == request_id
        ));
    }

    #[expect(
        clippy::panic,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn absent(&mut self) -> Event {
        loop {
            match self.next() {
                event @ Event::FamilyAbsent { .. } => return event,
                Event::RootExited { .. } => {}
                Event::Error { .. } => {
                    panic!("successful family cleanup cannot contain uncertainty")
                }
                _ => panic!("unexpected family cleanup event"),
            }
        }
    }

    fn acknowledge(&mut self) {
        self.send(Command::Acknowledge {
            execution_id: self.execution.clone(),
        });
        self.wait();
    }

    #[expect(
        clippy::unwrap_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn wait(&mut self) {
        let until = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                assert!(status.success(), "helper exits successfully after owned absence");
                return;
            }
            assert!(Instant::now() < until, "bounded helper termination");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if self.process.try_wait().ok().flatten().is_none() {
            if let Some(input) = &mut self.input {
                let _ = write_command(
                    input,
                    &Command::Signal {
                        execution_id: self.execution.clone(),
                        request_id: 1,
                        signal: libc::SIGKILL,
                    },
                );
            }
            self.input.take();
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                if self.process.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}

struct Sentinel(Child);

impl Sentinel {
    #[expect(
        clippy::expect_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn start() -> Self {
        Self(Process::new("/bin/sleep").arg("30").spawn().expect("external sentinel"))
    }

    #[expect(
        clippy::unwrap_used,
        reason = "A failed native fixture or contract assertion must terminate the test immediately"
    )]
    fn assert_alive(&mut self) {
        assert!(self.0.try_wait().unwrap().is_none());
    }
}

impl Drop for Sentinel {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn recorded(path: &Path) -> Identity {
    let until = Instant::now() + WAIT;
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            let fields: Vec<_> = value.split_whitespace().collect();
            if let [pid, start_time] = fields.as_slice() {
                return Identity {
                    pid: pid.parse().unwrap(),
                    start_time: start_time.parse().unwrap(),
                };
            }
        }
        assert!(Instant::now() < until, "native child positive control");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn pin(identity: Identity) -> OwnedFd {
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid as i32, 0) };
    assert!(raw >= 0, "pin exact native positive control");
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", identity.pid)).unwrap();
    let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().collect();
    assert_eq!(fields[19].parse::<u64>().unwrap(), identity.start_time);
    unsafe { OwnedFd::from_raw_fd(raw as i32) }
}

fn assert_dead(descriptor: &OwnedFd) {
    let mut poll = libc::pollfd {
        fd: descriptor.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(
        unsafe { libc::poll(&mut poll, 1, WAIT.as_millis() as i32) },
        1,
        "exact native child must be dead"
    );
    assert_ne!(poll.revents & libc::POLLIN, 0);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn held_child_waits_for_matching_release_and_preserves_status_and_data() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("effect");
    let mut helper = Harness::start();
    helper.initialize("effect", &marker, None, None);
    helper.send(Command::Release {
        execution_id: ExecutionId::new("other").unwrap(),
    });
    assert!(matches!(
        helper.next(),
        Event::Error {
            code: ErrorCode::WrongExecution,
            uncertain: Certainty::Certain,
            ..
        }
    ));
    helper.send(Command::Status {
        execution_id: helper.execution.clone(),
    });
    assert!(matches!(helper.next(), Event::Status { phase: Phase::Held, .. }));
    assert!(!marker.exists());
    helper.release();
    assert!(matches!(
        helper.absent(),
        Event::FamilyAbsent {
            released: ReleaseState::Released,
            root_exit_code: Some(7),
            root_signal: None,
            timed_out: TimeoutState::WithinDeadline,
            canceled: Cancellation::Active,
            stdout_truncated: Truncation::Complete,
            stderr_truncated: Truncation::Complete,
            ..
        }
    ));
    assert!(marker.exists());
    assert!(helper.output.starts_with(b"ordinary stdout\n"));
    assert!(
        helper
            .output
            .windows(b"\"kind\":\"family_absent\"".len())
            .any(|bytes| bytes == b"\"kind\":\"family_absent\"")
    );
    assert_eq!(helper.errors, b"ordinary stderr\n");
    assert!(
        helper.process.try_wait().unwrap().is_none(),
        "anchor remains until acknowledgement"
    );
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn control_eof_before_release_never_executes_user_work() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("effect");
    let mut helper = Harness::start();
    let root = pin(helper.initialize("effect", &marker, None, None));
    helper.input.take();
    assert!(matches!(
        helper.absent(),
        Event::FamilyAbsent {
            released: ReleaseState::Held,
            root_exit_code: None,
            root_signal: None,
            canceled: Cancellation::Canceled,
            ..
        }
    ));
    assert_dead(&root);
    assert!(!marker.exists());
    helper.wait();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn held_cancellation_and_release_deadline_never_execute_user_work() {
    for timeout in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("effect");
        let mut helper = Harness::start();
        let root = pin(helper.initialize("effect", &marker, None, timeout.then_some(50)));
        if !timeout {
            helper.signal(libc::SIGKILL);
        }
        assert!(
            matches!(helper.absent(), Event::FamilyAbsent { released: ReleaseState::Held,
            root_exit_code: None, root_signal: None, timed_out, canceled: Cancellation::Canceled, .. } if bool::from(timed_out) == timeout)
        );
        assert_dead(&root);
        assert!(!marker.exists());
        helper.acknowledge();
    }
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn hard_cancel_reaps_double_forked_new_session_with_empty_environment_and_preserves_sentinel() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("escaped");
    let mut sentinel = Sentinel::start();
    let mut helper = Harness::start();
    let root = pin(helper.initialize("escape", &marker, None, None));
    helper.release();
    let escaped = pin(recorded(&marker));
    sentinel.assert_alive();
    helper.signal(libc::SIGKILL);
    assert!(
        matches!(helper.absent(), Event::FamilyAbsent { released: ReleaseState::Released,
        root_signal: Some(libc::SIGKILL), canceled: Cancellation::Canceled, reaped_count, .. } if reaped_count >= 2)
    );
    assert_dead(&root);
    assert_dead(&escaped);
    sentinel.assert_alive();
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn clone_children_without_sigchld_are_included_in_absence() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("clone");
    let mut helper = Harness::start();
    helper.initialize("clone", &marker, None, None);
    helper.release();
    let child = pin(recorded(&marker));
    helper.signal(libc::SIGKILL);
    assert!(
        matches!(helper.absent(), Event::FamilyAbsent { reaped_count, canceled: Cancellation::Canceled, .. } if reaped_count >= 2)
    );
    assert_dead(&child);
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn exited_root_status_is_retained_until_escaped_family_is_absent() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("escaped");
    let mut helper = Harness::start();
    let root = pin(helper.initialize("escape-root-exits", &marker, None, None));
    helper.release();
    let child = pin(recorded(&marker));
    assert!(matches!(
        helper.next(),
        Event::RootExited {
            exit_code: Some(7),
            signal: None,
            ..
        }
    ));
    assert_dead(&root);
    helper.send(Command::Status {
        execution_id: helper.execution.clone(),
    });
    assert!(matches!(
        helper.next(),
        Event::Status {
            phase: Phase::Running,
            ..
        }
    ));
    helper.signal(libc::SIGKILL);
    assert!(
        matches!(helper.absent(), Event::FamilyAbsent { root_exit_code: Some(7), root_signal: None,
        reaped_count, .. } if reaped_count >= 2)
    );
    assert_dead(&child);
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn term_and_interrupt_preserve_genuine_success_exit() {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("ready");
        let mut helper = Harness::start();
        helper.initialize("term", &marker, None, None);
        helper.release();
        let root = pin(recorded(&marker));
        helper.signal(signal);
        assert!(matches!(
            helper.absent(),
            Event::FamilyAbsent {
                root_exit_code: Some(0),
                root_signal: None,
                canceled: Cancellation::Active,
                ..
            }
        ));
        assert_dead(&root);
        helper.acknowledge();
    }
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn graceful_root_exit_then_hard_escalation_keeps_original_status() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("escaped");
    let mut helper = Harness::start();
    helper.initialize("term-tree", &marker, None, None);
    helper.release();
    let child = pin(recorded(&marker));
    helper.signal(libc::SIGTERM);
    assert!(matches!(
        helper.next(),
        Event::RootExited {
            exit_code: Some(0),
            signal: None,
            ..
        }
    ));
    helper.signal(libc::SIGKILL);
    assert!(matches!(
        helper.absent(),
        Event::FamilyAbsent {
            root_exit_code: Some(0),
            root_signal: None,
            canceled: Cancellation::Canceled,
            ..
        }
    ));
    assert_dead(&child);
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn hard_cancel_during_fork_and_thread_churn_reaps_adopted_children() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("ready");
    let mut helper = Harness::start();
    helper.initialize("churn", &marker, None, None);
    helper.release();
    let root = pin(recorded(&marker));
    std::thread::sleep(Duration::from_millis(40));
    helper.signal(libc::SIGKILL);
    assert!(
        matches!(helper.absent(), Event::FamilyAbsent { reaped_count, canceled: Cancellation::Canceled, .. } if reaped_count >= 2)
    );
    assert_dead(&root);
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn command_deadline_uses_owned_family_drain() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("escaped");
    let mut helper = Harness::start();
    helper.initialize("escape", &marker, Some(500), None);
    helper.release();
    let child = pin(recorded(&marker));
    assert!(matches!(
        helper.absent(),
        Event::FamilyAbsent {
            timed_out: TimeoutState::TimedOut,
            canceled: Cancellation::Canceled,
            ..
        }
    ));
    assert_dead(&child);
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn completed_family_signal_is_a_noop() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("effect");
    let mut helper = Harness::start();
    helper.initialize("effect", &marker, None, None);
    helper.release();
    helper.absent();
    helper.signal(libc::SIGKILL);
    assert!(matches!(
        helper.next(),
        Event::Status {
            phase: Phase::Absent,
            ..
        }
    ));
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn exec_failure_is_reported_before_terminal_absence_and_preserves_real_exit() {
    let mut helper = Harness::start();
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec!["/sandbox-executable-that-does-not-exist".into()],
        env: BTreeMap::new(),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    });
    assert!(matches!(helper.next(), Event::Ready { .. }));
    helper.release();
    let mut exec_failed = false;
    loop {
        match helper.next() {
            Event::Error {
                code: ErrorCode::ExecFailed,
                uncertain: Certainty::Certain,
                ..
            } => {
                assert!(!exec_failed);
                exec_failed = true;
            }
            Event::RootExited {
                exit_code: Some(127),
                signal: None,
                ..
            } => {}
            Event::FamilyAbsent {
                root_exit_code: Some(127),
                root_signal: None,
                ..
            } => break,
            _ => panic!("exact executable failure contract"),
        }
    }
    assert!(exec_failed);
    helper.send(Command::Status {
        execution_id: helper.execution.clone(),
    });
    assert!(matches!(
        helper.next(),
        Event::Status {
            phase: Phase::Absent,
            ..
        }
    ));
    helper.acknowledge();
}

#[test]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn output_backpressure_cannot_prevent_control_eof_from_killing_root() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("flood");
    let execution = ExecutionId::new("backpressure").unwrap();
    let mut process = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"))
        .args(["--execution-id", execution.as_str()])
        .env_clear()
        .envs(coverage_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(process.stdout.take().unwrap());
    let mut input = process.stdin.take().unwrap();
    assert!(matches!(read_event(&mut output).unwrap(), Some(Event::Hello { .. })));
    write_command(
        &mut input,
        &Command::Init {
            version: VERSION,
            execution_id: execution.clone(),
            argv: vec![
                env!("CARGO_BIN_EXE_supervisor-test-fixture").into(),
                "flood".into(),
                marker.to_string_lossy().into_owned(),
            ],
            env: workload_environment(BTreeMap::new()),
            cwd: None,
            timeout_ms: None,
            release_timeout_ms: None,
        },
    )
    .unwrap();
    let root = match read_event(&mut output).unwrap() {
        Some(Event::Ready { root, .. }) => pin(root),
        _ => panic!("native root is held"),
    };
    write_command(
        &mut input,
        &Command::Release {
            execution_id: execution.clone(),
        },
    )
    .unwrap();
    assert!(matches!(read_event(&mut output).unwrap(), Some(Event::Released { .. })));
    recorded(&marker);
    std::thread::sleep(Duration::from_millis(200));
    drop(input);
    assert_dead(&root);
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(event)) = read_event(&mut output) {
            if matches!(event, Event::FamilyAbsent { .. }) {
                let _ = send.send(event);
                break;
            }
        }
    });
    assert!(matches!(
        receive.recv_timeout(WAIT),
        Ok(Event::FamilyAbsent {
            canceled: Cancellation::Canceled,
            stdout_truncated: Truncation::Truncated,
            ..
        })
    ));
    let until = Instant::now() + WAIT;
    loop {
        if let Some(status) = process.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= until {
            let _ = process.kill();
            panic!("bounded helper exit");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn missing_kernel_prerequisites_refuse_probe() {
    for blocked in [
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_send_signal,
        libc::SYS_prctl,
        libc::SYS_close_range,
    ] {
        let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
        command.arg("--probe").env_clear().envs(coverage_environment());
        unsafe {
            command.pre_exec(move || {
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
                        jf: 1,
                        k: blocked as u32,
                    },
                    libc::sock_filter {
                        code: (libc::BPF_RET | libc::BPF_K) as u16,
                        jt: 0,
                        jf: 0,
                        k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
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
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0
                    || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let result = command.output().expect("run prerequisite refusal control");
        assert_eq!(result.status.code(), Some(78));
        assert!(matches!(
            read_event(&mut result.stdout.as_slice()).unwrap(),
            Some(Event::Probe {
                supported: PlatformSupport::Unsupported,
                ..
            })
        ));
    }
}

#[test]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn native_signal_request_retries_do_not_repeat_effects_or_change_signal() {
    fn signal(helper: &mut Harness, request_id: u64, signal: i32) -> Event {
        helper.send(Command::Signal {
            execution_id: helper.execution.clone(),
            request_id,
            signal,
        });
        helper.next()
    }

    fn delivered(event: &Event, expected_id: u64, expected_signal: i32) -> bool {
        matches!(event, Event::SignalResult { request_id, signal, delivered: Delivery::Delivered, .. }
            if *request_id == expected_id && *signal == expected_signal)
    }

    fn rejected(event: &Event) -> bool {
        matches!(
            event,
            Event::Error {
                code: ErrorCode::InvalidCommand,
                uncertain: Certainty::Certain,
                ..
            }
        )
    }

    fn count(path: &Path) -> usize {
        std::fs::read(path).map(|bytes| bytes.len()).unwrap_or(0)
    }

    fn wait_count(path: &Path, minimum: usize) -> bool {
        let until = Instant::now() + WAIT;
        while count(path) < minimum && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        count(path) >= minimum
    }

    let directory = tempfile::tempdir().unwrap();
    let first_receipt = directory.path().join("usr1");
    let second_receipt = directory.path().join("usr2");
    let ready = directory.path().join("ready");
    let release = directory.path().join("release");
    let mut sentinel = Sentinel::start();
    let sentinel_pid = sentinel.0.id();
    let sentinel_stat = std::fs::read_to_string(format!("/proc/{sentinel_pid}/stat")).unwrap();
    let (_, sentinel_fields) = sentinel_stat.rsplit_once(')').unwrap();
    let sentinel_identity = pin(Identity {
        pid: sentinel_pid,
        start_time: sentinel_fields.split_whitespace().nth(19).unwrap().parse().unwrap(),
    });
    let mut helper = Harness::start();
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "trap 'printf A >> \"$1\"' USR1; trap 'printf B >> \"$2\"' USR2; printf 'signal replay stdout\\n'; printf 'signal replay stderr\\n' >&2; printf ready > \"$3\"; while [ ! -f \"$4\" ]; do :; done; exit 7".into(),
            "sh".into(),
            first_receipt.to_string_lossy().into_owned(),
            second_receipt.to_string_lossy().into_owned(),
            ready.to_string_lossy().into_owned(),
            release.to_string_lossy().into_owned(),
        ],
        env: workload_environment(BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())])),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    });
    let root = match helper.next() {
        Event::Ready { root, .. } => pin(root),
        _ => panic!("signal replay fixture requires a held native root"),
    };
    helper.release();
    let until = Instant::now() + WAIT;
    while !ready.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    let root_ready = ready.exists();
    let sentinel_before = sentinel.0.try_wait().unwrap().is_none();
    let first = signal(&mut helper, 41, libc::SIGUSR1);
    let first_effect = wait_count(&first_receipt, 1);
    let duplicate = signal(&mut helper, 41, libc::SIGUSR1);
    std::thread::sleep(Duration::from_millis(100));
    let collision = signal(&mut helper, 41, libc::SIGUSR2);
    std::thread::sleep(Duration::from_millis(100));
    let before_fresh = count(&first_receipt);
    let fresh = signal(&mut helper, 1007, libc::SIGUSR1);
    let fresh_effect = wait_count(&first_receipt, before_fresh + 1);
    let stale = signal(&mut helper, 41, libc::SIGUSR1);
    std::thread::sleep(Duration::from_millis(100));
    let fresh_duplicate = signal(&mut helper, 1007, libc::SIGUSR1);
    std::thread::sleep(Duration::from_millis(100));
    helper.send(Command::Signal {
        execution_id: ExecutionId::new("foreign-execution").unwrap(),
        request_id: 9000,
        signal: libc::SIGUSR2,
    });
    let foreign = helper.next();
    let before_forward = count(&second_receipt);
    let forward = signal(&mut helper, 1008, libc::SIGUSR2);
    let forward_effect = wait_count(&second_receipt, before_forward + 1);
    let released = std::fs::write(&release, b"release").is_ok();
    let absence = helper.absent();
    assert_dead(&root);
    let usr1_effects = count(&first_receipt);
    let usr2_effects = count(&second_receipt);
    let output_preserved = helper.output == b"signal replay stdout\n" && helper.errors == b"signal replay stderr\n";
    helper.acknowledge();
    let sentinel_after = sentinel.0.try_wait().unwrap().is_none();
    drop(sentinel);
    assert_dead(&sentinel_identity);

    assert!(released, "the owned native fixture must be released for cleanup");
    assert!(
        sentinel_before && sentinel_after,
        "native signal retries must preserve the independent sentinel"
    );
    assert!(
        matches!(
            absence,
            Event::FamilyAbsent {
                released: ReleaseState::Released,
                root_exit_code: Some(7),
                root_signal: None,
                timed_out: TimeoutState::WithinDeadline,
                canceled: Cancellation::Active,
                ..
            }
        ),
        "native cleanup must retain genuine user completion"
    );
    assert!(
        root_ready && first_effect && fresh_effect && forward_effect,
        "fresh nonconsecutive signal requests require real native effect controls"
    );
    assert!(
        delivered(&first, 41, libc::SIGUSR1)
            && delivered(&fresh, 1007, libc::SIGUSR1)
            && delivered(&forward, 1008, libc::SIGUSR2),
        "monotonic signal request IDs need not start at one or be consecutive"
    );
    assert!(
        delivered(&duplicate, 41, libc::SIGUSR1) && delivered(&fresh_duplicate, 1007, libc::SIGUSR1),
        "an identical retry must replay the original signal result"
    );
    assert!(
        rejected(&collision),
        "reusing a request ID with a foreign signal must be refused"
    );
    assert!(rejected(&stale), "an older request ID must not deliver another signal");
    assert!(
        matches!(
            foreign,
            Event::Error {
                code: ErrorCode::WrongExecution,
                uncertain: Certainty::Certain,
                ..
            }
        ),
        "foreign execution requests must be refused before changing request custody"
    );
    assert_eq!(
        usr1_effects, 2,
        "identical and stale retries must not repeat a nonterminal native effect"
    );
    assert_eq!(
        usr2_effects, 1,
        "a request-ID collision must not deliver the foreign signal"
    );
    assert!(output_preserved, "request custody must preserve ordinary native output");
}

#[path = "native_contract/overflow_tests.rs"]
mod overflow_tests;

#[path = "native_contract/fixture_tests.rs"]
mod fixture_tests;

#[path = "native_contract/syscall_notifications.rs"]
mod syscall_notifications;

fn coverage_environment() -> BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    #[cfg(coverage)]
    {
        trogon_std::env::SystemEnv
            .var_os("LLVM_PROFILE_FILE")
            .map(|path| {
                let path = if trogon_std::env::SystemEnv.var_os("RUST_COVERAGE_CONTINUOUS").as_deref()
                    == Some(std::ffi::OsStr::new("1"))
                {
                    std::path::PathBuf::from(path)
                        .with_file_name(format!("supervisor-{}-%p%c.profraw", std::process::id()))
                        .into_os_string()
                } else {
                    path
                };
                BTreeMap::from([("LLVM_PROFILE_FILE".into(), path)])
            })
            .unwrap_or_default()
    }
    #[cfg(not(coverage))]
    BTreeMap::new()
}

fn workload_environment(mut environment: BTreeMap<String, String>) -> BTreeMap<String, String> {
    for (key, value) in coverage_environment() {
        if let (Ok(key), Ok(value)) = (key.into_string(), value.into_string()) {
            environment.insert(key, value);
        }
    }
    environment
}

#[test]
fn legacy_fixture_profiling_preserves_exact_application_output() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("effect");
    let mut command = Process::new(env!("CARGO_BIN_EXE_supervisor-test-fixture"));
    command.arg("effect").arg(&marker).env_clear();
    #[cfg(coverage)]
    if let Some(profile) = trogon_std::env::SystemEnv.var_os("LLVM_PROFILE_FILE") {
        command.env(
            "LLVM_PROFILE_FILE",
            std::path::PathBuf::from(profile)
                .with_file_name(format!("legacy-fixture-{}-%p-%m.profraw", std::process::id())),
        );
    }
    let output = command.output()?;
    assert_eq!(output.status.code(), Some(7));
    assert!(marker.exists());
    assert_eq!(
        output.stdout,
        b"ordinary stdout\n{\"kind\":\"family_absent\",\"execution_id\":\"contract\"}\n"
    );
    assert_eq!(output.stderr, b"ordinary stderr\n");
    Ok(())
}

#[test]
fn path_resolution_uses_the_explicit_child_environment_and_working_directory() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("run");
    std::fs::write(&executable, b"#!/bin/sh\nprintf 'child:%s' \"$TOKEN\"\n")?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
    for path in [
        String::new(),
        ".".into(),
        directory.path().to_string_lossy().into_owned(),
    ] {
        let mut helper = Harness::start();
        helper.send(Command::Init {
            version: VERSION,
            execution_id: helper.execution.clone(),
            argv: vec!["run".into()],
            env: workload_environment(BTreeMap::from([
                ("PATH".into(), path),
                ("TOKEN".into(), "explicit".into()),
            ])),
            cwd: Some(directory.path().to_string_lossy().into_owned()),
            timeout_ms: None,
            release_timeout_ms: None,
        });
        assert!(matches!(helper.next(), Event::Ready { .. }));
        helper.release();
        assert!(matches!(
            helper.absent(),
            Event::FamilyAbsent {
                root_exit_code: Some(0),
                canceled: Cancellation::Active,
                ..
            }
        ));
        assert_eq!(helper.output, b"child:explicit");
        helper.acknowledge();
    }
    let mut helper = Harness::start();
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec!["true".into()],
        env: BTreeMap::new(),
        cwd: Some(".".into()),
        timeout_ms: None,
        release_timeout_ms: None,
    });
    assert!(matches!(helper.next(), Event::Ready { .. }));
    helper.release();
    assert!(matches!(
        helper.absent(),
        Event::FamilyAbsent {
            root_exit_code: Some(0),
            ..
        }
    ));
    helper.acknowledge();
    Ok(())
}

#[test]
fn malformed_frames_and_wrong_phase_commands_leave_the_control_channel_usable() -> Result<(), Box<dyn std::error::Error>>
{
    let mut helper = Harness::start();
    for bytes in [
        b"not-json\n".to_vec(),
        [
            vec![b'x'; sandbox_command_supervisor::protocol::MAX_FRAME_BYTES + 10],
            vec![b'\n'],
        ]
        .concat(),
    ] {
        helper
            .input
            .as_mut()
            .ok_or_else(|| std::io::Error::other("open control"))?
            .write_all(&bytes)?;
        assert!(matches!(
            helper.next(),
            Event::Error {
                code: ErrorCode::InvalidFrame,
                uncertain: Certainty::Certain,
                ..
            }
        ));
    }
    for command in [
        Command::Release {
            execution_id: helper.execution.clone(),
        },
        Command::Acknowledge {
            execution_id: helper.execution.clone(),
        },
        Command::Signal {
            execution_id: helper.execution.clone(),
            request_id: 1,
            signal: libc::SIGTERM,
        },
    ] {
        helper.send(command);
        assert!(matches!(
            helper.next(),
            Event::Error {
                code: ErrorCode::WrongPhase,
                uncertain: Certainty::Certain,
                ..
            }
        ));
    }
    helper.send(Command::Status {
        execution_id: helper.execution.clone(),
    });
    assert!(matches!(
        helper.next(),
        Event::Status {
            phase: Phase::AwaitingInit,
            ..
        }
    ));
    helper.input.take();
    helper.wait();
    Ok(())
}

#[test]
fn unavailable_working_directory_refuses_launch_and_reaps_the_setup_child() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("effect");
    let mut helper = Harness::start();
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec![
            env!("CARGO_BIN_EXE_supervisor-test-fixture").into(),
            "effect".into(),
            marker.to_string_lossy().into_owned(),
        ],
        env: workload_environment(BTreeMap::new()),
        cwd: Some(directory.path().join("absent").to_string_lossy().into_owned()),
        timeout_ms: None,
        release_timeout_ms: None,
    });
    assert!(matches!(
        helper.next(),
        Event::Error {
            code: ErrorCode::SetupFailed,
            uncertain: Certainty::Uncertain,
            ..
        }
    ));
    assert!(
        matches!(helper.absent(), Event::FamilyAbsent { released: ReleaseState::Held, canceled: Cancellation::Canceled, reaped_count, .. } if reaped_count >= 1)
    );
    assert!(!marker.exists());
    helper.acknowledge();
    Ok(())
}

#[test]
fn native_probe_and_cli_argument_statuses_match_the_wire_contract() -> Result<(), Box<dyn std::error::Error>> {
    let output = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"))
        .arg("--probe")
        .env_clear()
        .envs(coverage_environment())
        .output()?;
    assert!(output.status.success());
    assert!(matches!(
        read_event(&mut output.stdout.as_slice())?,
        Some(Event::Probe {
            supported: PlatformSupport::Supported,
            version: VERSION
        })
    ));
    for args in [Vec::<&str>::new(), vec!["--unknown"], vec!["--execution-id", ""]] {
        let output = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"))
            .args(args)
            .env_clear()
            .envs(coverage_environment())
            .output()?;
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
    }
    Ok(())
}

fn deny_syscall(command: &mut Process, blocked: libc::c_long) {
    unsafe {
        command.pre_exec(move || {
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
                    jf: 1,
                    k: blocked as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
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
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0
                || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[test]
fn refused_pipe_fork_and_session_syscalls_preserve_owned_cleanup_and_never_exec()
-> Result<(), Box<dyn std::error::Error>> {
    for (blocked, expected, minimum_reaped) in [
        (libc::SYS_pipe2, ErrorCode::IoFailure, 0),
        (libc::SYS_clone, ErrorCode::SetupFailed, 0),
        (libc::SYS_setsid, ErrorCode::SetupFailed, 1),
    ] {
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("effect");
        let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
        deny_syscall(&mut command, blocked);
        let mut helper = Harness::start_command(command);
        helper.send(Command::Init {
            version: VERSION,
            execution_id: helper.execution.clone(),
            argv: vec![
                env!("CARGO_BIN_EXE_supervisor-test-fixture").into(),
                "effect".into(),
                marker.to_string_lossy().into_owned(),
            ],
            env: workload_environment(BTreeMap::new()),
            cwd: None,
            timeout_ms: None,
            release_timeout_ms: None,
        });
        assert!(
            matches!(helper.next(), Event::Error { code, uncertain: Certainty::Uncertain, .. } if code == expected)
        );
        assert!(
            matches!(helper.absent(), Event::FamilyAbsent { released: ReleaseState::Held, canceled: Cancellation::Canceled, reaped_count, .. } if reaped_count >= minimum_reaped)
        );
        assert!(!marker.exists());
        helper.acknowledge();
    }
    Ok(())
}

#[test]
fn unavailable_nonblocking_io_refuses_startup_before_hello() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
    command
        .args(["--execution-id", "contract"])
        .env_clear()
        .envs(coverage_environment());
    deny_syscall(&mut command, libc::SYS_fcntl);
    let output = command.output()?;
    assert_eq!(output.status.code(), Some(78));
    assert!(matches!(
        read_event(&mut output.stdout.as_slice())?,
        Some(Event::Error {
            code: ErrorCode::IoFailure,
            uncertain: Certainty::Certain,
            ..
        })
    ));
    Ok(())
}
