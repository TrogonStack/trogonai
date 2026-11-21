use super::parse_children;
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use trogon_std::env::ReadEnv;

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

mod containment_tests;
mod worker_tests;
use worker_tests as worker;

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

pub(super) fn continuous_profile() -> Option<(&'static str, std::ffi::OsString)> {
    if super::SystemEnv.var_os("RUST_COVERAGE_CONTINUOUS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    let profile = super::SystemEnv.var_os("LLVM_PROFILE_FILE")?;
    let directory = std::path::Path::new(&profile).parent()?;
    Some((
        "LLVM_PROFILE_FILE",
        directory
            .join(format!("kernel-{}-%p%c.profraw", std::process::id()))
            .into_os_string(),
    ))
}

pub(super) fn isolated(name: &str, body: impl FnOnce() -> TestResult) -> TestResult {
    isolated_with_timeout(name, Duration::from_secs(5), body)
}

pub(super) fn isolated_with_timeout(name: &str, timeout: Duration, body: impl FnOnce() -> TestResult) -> TestResult {
    if super::SystemEnv.var_os("SUPERVISOR_KERNEL_TEST").as_deref() == Some(std::ffi::OsStr::new(name)) {
        assert_eq!(unsafe { libc::syscall(libc::SYS_gettid) }, unsafe { libc::getpid() }
            as libc::c_long);
        return body();
    }
    worker::run(name, timeout)
}

pub(super) fn isolated_process(name: &str, body: impl FnOnce() -> TestResult) -> TestResult {
    isolated(name, body)
}

pub(crate) fn worker_main() -> i32 {
    if unsafe { libc::syscall(libc::SYS_gettid) } != unsafe { libc::getpid() } as libc::c_long {
        return 70;
    }
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let [name, source] = arguments.as_slice() else {
        return 64;
    };
    let Some(name) = name.to_str() else {
        return 64;
    };
    if super::SystemEnv.var_os("SUPERVISOR_KERNEL_TEST").as_deref() != Some(std::ffi::OsStr::new(name)) {
        return 64;
    }
    if source != std::ffi::OsStr::new(&worker::source_fingerprint()) {
        eprintln!(
            "stale kernel worker; rebuild with: cargo build -p sandbox-command-supervisor --features test-support --bin supervisor-kernel-test-worker"
        );
        return 65;
    }
    let result = std::panic::catch_unwind(|| {
        dispatch_worker(name)
            .or_else(|| super::state_tests::dispatch_worker(name))
            .or_else(|| super::native_child::tests::dispatch_worker(name))
            .or_else(|| super::writer::tests::dispatch_worker(name))
            .or_else(|| super::signal_dispatch::tests::dispatch_worker(name))
            .or_else(|| containment_tests::dispatch_worker(name))
    });
    match result {
        Ok(Some(Ok(()))) => 0,
        Ok(Some(Err(error))) => {
            eprintln!("{error}");
            1
        }
        Ok(None) => 64,
        Err(_) => 1,
    }
}

pub(super) fn refuse_syscall(syscall: libc::c_long, error: i32) -> TestResult {
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
            k: syscall as u32,
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
    install_filter(&mut filter)
}

pub(super) fn refuse_stdout_frame_write(length: usize) -> TestResult {
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
            jf: 5,
            k: libc::SYS_write as u32,
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
            jf: 3,
            k: libc::STDOUT_FILENO as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 32,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: length as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ERRNO,
        },
        libc::sock_filter {
            code: (libc::BPF_RET | libc::BPF_K) as u16,
            jt: 0,
            jf: 0,
            k: libc::SECCOMP_RET_ALLOW,
        },
    ];
    install_filter(&mut filter)
}

fn install_filter(filter: &mut [libc::sock_filter]) -> TestResult {
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0
        || unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

const PRODUCTION_LIMIT: usize = 4096;

fn identifiers(count: usize) -> Vec<u8> {
    (1..=count)
        .map(|identity| format!("{identity} "))
        .collect::<String>()
        .into_bytes()
}

#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
pub(in crate::linux) fn worker_child_parser_preserves_below_and_exact_production_limit() {
    for count in [PRODUCTION_LIMIT - 1, PRODUCTION_LIMIT] {
        let snapshot = parse_children(&identifiers(count), false, PRODUCTION_LIMIT)
            .unwrap_or_else(|_| panic!("complete positive kernel IDs"));
        assert_eq!(snapshot.children.len(), count);
        assert_eq!(snapshot.children.first(), Some(&1));
        assert_eq!(snapshot.children.last(), Some(&(count as i32)));
        assert!(!snapshot.has_more);
    }
}

#[test]
fn child_parser_preserves_below_and_exact_production_limit() {
    worker_child_parser_preserves_below_and_exact_production_limit()
}

#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
pub(in crate::linux) fn worker_child_parser_retains_bounded_prefix_above_production_limit() {
    let snapshot = parse_children(&identifiers(PRODUCTION_LIMIT + 1), false, PRODUCTION_LIMIT)
        .unwrap_or_else(|_| panic!("complete bounded prefix"));
    assert_eq!(snapshot.children.len(), PRODUCTION_LIMIT);
    assert_eq!(snapshot.children.last(), Some(&(PRODUCTION_LIMIT as i32)));
    assert!(snapshot.has_more);
}

#[test]
fn child_parser_retains_bounded_prefix_above_production_limit() {
    worker_child_parser_retains_bounded_prefix_above_production_limit()
}

#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
pub(in crate::linux) fn worker_child_parser_never_authorizes_a_truncated_pid_token() {
    let mut bytes = identifiers(PRODUCTION_LIMIT - 1);
    bytes.extend_from_slice(b"12345");
    for truncated in [false, true] {
        let snapshot = parse_children(&bytes, truncated, PRODUCTION_LIMIT)
            .unwrap_or_else(|_| panic!("complete prefix survives incomplete tail"));
        assert_eq!(snapshot.children.len(), PRODUCTION_LIMIT - 1);
        assert_eq!(snapshot.children.last(), Some(&((PRODUCTION_LIMIT - 1) as i32)));
        assert!(!snapshot.children.contains(&12345));
        assert!(snapshot.has_more);
    }
    let snapshot =
        parse_children(b"7", true, PRODUCTION_LIMIT).unwrap_or_else(|_| panic!("incomplete first token is uncertain"));
    assert!(snapshot.children.is_empty());
    assert!(snapshot.has_more);
}

#[test]
fn child_parser_never_authorizes_a_truncated_pid_token() {
    worker_child_parser_never_authorizes_a_truncated_pid_token()
}

#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
pub(in crate::linux) fn worker_child_parser_keeps_exhausted_whitespace_boundary_uncertain() {
    let snapshot =
        parse_children(b"71 72 ", true, PRODUCTION_LIMIT).unwrap_or_else(|_| panic!("complete tokens at byte bound"));
    assert_eq!(snapshot.children, [71, 72]);
    assert!(snapshot.has_more);
    let snapshot =
        parse_children(b"", false, PRODUCTION_LIMIT).unwrap_or_else(|_| panic!("complete empty kernel listing"));
    assert!(snapshot.children.is_empty());
    assert!(!snapshot.has_more);
}

#[test]
fn child_parser_keeps_exhausted_whitespace_boundary_uncertain() {
    worker_child_parser_keeps_exhausted_whitespace_boundary_uncertain()
}

pub(in crate::linux) fn worker_child_parser_refuses_malformed_and_nonpositive_complete_ids() {
    for bytes in [
        b"0 ".as_slice(),
        b"-1 ",
        b"+1 ",
        b"2147483648 ",
        b"1e3 ",
        b"1\0 ",
        b"a ",
        b"1 \xff ",
    ] {
        assert!(parse_children(bytes, false, PRODUCTION_LIMIT).is_err());
    }
}

#[test]
fn child_parser_refuses_malformed_and_nonpositive_complete_ids() {
    worker_child_parser_refuses_malformed_and_nonpositive_complete_ids()
}

pub(in crate::linux) fn worker_invalid_descriptors_and_process_handles_preserve_kernel_errors() -> TestResult {
    assert_eq!(super::nonblocking(-1), Err(super::ErrorCode::IoFailure));
    assert!(matches!(super::pidfd(-1), Err(super::ErrorCode::IdentityUnknown)));
    let (read, _) = super::pipe()?;
    assert_eq!(super::signal_pidfd(&read, 0), Err(super::ErrorCode::IdentityUnknown));
    Ok(())
}

#[test]
fn invalid_descriptors_and_process_handles_preserve_kernel_errors() -> TestResult {
    worker_invalid_descriptors_and_process_handles_preserve_kernel_errors()
}

pub(in crate::linux) fn worker_syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors() -> TestResult
{
    isolated(
        "linux::tests::syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors",
        || {
            refuse_syscall(libc::SYS_pipe2, libc::EMFILE)?;
            assert!(matches!(super::pipe(), Err(super::ErrorCode::IoFailure)));
            Ok(())
        },
    )
}

#[test]
fn syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors() -> TestResult {
    worker_syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors()
}

pub(in crate::linux) fn worker_interrupted_readiness_is_unknown_without_becoming_a_terminal_error() -> TestResult {
    isolated(
        "linux::tests::interrupted_readiness_is_unknown_without_becoming_a_terminal_error",
        || {
            let (read, _) = super::pipe()?;
            refuse_syscall(POLL_SYSCALL, libc::EINTR)?;
            assert_eq!(super::readable(&read), Ok(false));
            Ok(())
        },
    )
}

#[test]
fn interrupted_readiness_is_unknown_without_becoming_a_terminal_error() -> TestResult {
    worker_interrupted_readiness_is_unknown_without_becoming_a_terminal_error()
}

pub(in crate::linux) fn worker_failed_readiness_preserves_the_io_failure() -> TestResult {
    isolated("linux::tests::failed_readiness_preserves_the_io_failure", || {
        let (read, _) = super::pipe()?;
        refuse_syscall(POLL_SYSCALL, libc::EIO)?;
        assert_eq!(super::readable(&read), Err(super::ErrorCode::IoFailure));
        Ok(())
    })
}

#[test]
fn failed_readiness_preserves_the_io_failure() -> TestResult {
    worker_failed_readiness_preserves_the_io_failure()
}

pub(in crate::linux) fn worker_signal_callback_retains_the_kernel_signal() -> TestResult {
    isolated("linux::tests::signal_callback_retains_the_kernel_signal", || {
        unsafe { super::disposition(libc::SIGUSR1, super::remember_signal as *const () as libc::sighandler_t)? };
        assert_eq!(unsafe { libc::raise(libc::SIGUSR1) }, 0);
        assert_eq!(super::EXTERNAL_SIGNAL.swap(0, super::Ordering::Relaxed), libc::SIGUSR1);
        Ok(())
    })
}

#[test]
fn signal_callback_retains_the_kernel_signal() -> TestResult {
    worker_signal_callback_retains_the_kernel_signal()
}

pub(in crate::linux) fn worker_wait_status_distinguishes_signaled_roots_from_stopped_children() -> TestResult {
    isolated(
        "linux::tests::wait_status_distinguishes_signaled_roots_from_stopped_children",
        || {
            let mut child = Command::new("/bin/sh").args(["-c", "kill -STOP $$"]).spawn()?;
            let mut stopped = 0;
            let observed = unsafe { libc::waitpid(child.id() as i32, &mut stopped, libc::WUNTRACED) };
            let killed = child.kill();
            let status = child.wait()?;
            killed?;
            assert!(observed > 0);
            assert!(super::RootStatus::from_raw(stopped).is_none());
            let terminal = super::RootStatus::from_raw(status.into_raw())
                .ok_or_else(|| std::io::Error::other("terminal wait status"))?;
            assert_eq!(terminal.code, None);
            assert_eq!(terminal.signal, Some(libc::SIGKILL));
            Ok(())
        },
    )
}

#[test]
fn wait_status_distinguishes_signaled_roots_from_stopped_children() -> TestResult {
    worker_wait_status_distinguishes_signaled_roots_from_stopped_children()
}

pub(in crate::linux) fn worker_a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement() -> TestResult {
    isolated(
        "linux::tests::a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement",
        || {
            let mut child = Command::new("/bin/sh")
                .args(["-c", "read -r line"])
                .stdin(Stdio::piped())
                .spawn()?;
            let descriptor = super::pidfd(child.id() as i32);
            drop(child.stdin.take());
            child.wait()?;
            let descriptor = descriptor?;
            assert_eq!(super::signal_pidfd(&descriptor, 0), Ok(false));
            assert_eq!(super::readable(&descriptor), Ok(true));
            Ok(())
        },
    )
}

#[test]
fn a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement() -> TestResult {
    worker_a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement()
}

fn loop_supervisor() -> Result<super::Supervisor, crate::ExecutionIdError> {
    Ok(super::Supervisor::new(
        crate::ExecutionId::new("loop-contract")?,
        super::MAX_CHILDREN,
    ))
}

pub(in crate::linux) fn worker_closed_control_output_ends_an_absent_loop_without_reopening_cleanup() -> TestResult {
    isolated(
        "linux::tests::closed_control_output_ends_an_absent_loop_without_reopening_cleanup",
        || {
            let mut supervisor = loop_supervisor()?;
            supervisor.phase = super::Phase::Absent;
            supervisor.writer.closed = true;
            assert_eq!(supervisor.run_loop(), 74);
            assert!(supervisor.input_closed);
            assert!(!supervisor.canceled);
            assert!(supervisor.cleanup_started.is_none());
            Ok(())
        },
    )
}

#[test]
fn closed_control_output_ends_an_absent_loop_without_reopening_cleanup() -> TestResult {
    worker_closed_control_output_ends_an_absent_loop_without_reopening_cleanup()
}

pub(in crate::linux) fn worker_idle_initialization_deadline_preserves_open_input_and_returns_usage_status() -> TestResult
{
    isolated(
        "linux::tests::idle_initialization_deadline_preserves_open_input_and_returns_usage_status",
        || {
            let (read, _write) = super::pipe()?;
            assert_eq!(unsafe { libc::dup2(read.as_raw_fd(), 0) }, 0);
            super::nonblocking(0)?;
            let mut supervisor = loop_supervisor()?;
            supervisor.entered = Instant::now() - super::INIT_TIMEOUT;
            assert_eq!(supervisor.run_loop(), 64);
            assert!(!supervisor.input_closed);
            assert!(matches!(supervisor.phase, super::Phase::AwaitingInit));
            assert!(!supervisor.released);
            assert!(supervisor.root.is_none());
            Ok(())
        },
    )
}

#[test]
fn idle_initialization_deadline_preserves_open_input_and_returns_usage_status() -> TestResult {
    worker_idle_initialization_deadline_preserves_open_input_and_returns_usage_status()
}

pub(in crate::linux) fn worker_terminal_receipt_backpressure_expires_without_dropping_the_pending_frame() -> TestResult
{
    isolated(
        "linux::tests::terminal_receipt_backpressure_expires_without_dropping_the_pending_frame",
        || {
            let mut supervisor = loop_supervisor()?;
            supervisor.phase = super::Phase::Absent;
            supervisor.input_closed = true;
            supervisor.terminal_at = Some(Instant::now() - super::ACK_TIMEOUT);
            let event = super::Event::FamilyAbsent {
                execution_id: supervisor.execution.clone(),
                released: false.into(),
                root_exit_code: None,
                root_signal: None,
                timed_out: false.into(),
                canceled: true.into(),
                reaped_count: 0,
                stdout_truncated: false.into(),
                stderr_truncated: false.into(),
            };
            let bytes = super::encode_event(&event)?.len();
            supervisor.writer.queue(event)?;
            refuse_stdout_frame_write(bytes)?;
            assert_eq!(supervisor.run_loop(), 74);
            assert_eq!(supervisor.writer.pending.len(), 1);
            assert_eq!(supervisor.writer.control_bytes, bytes);
            assert!(!supervisor.writer.closed);
            assert!(!supervisor.acknowledged);
            Ok(())
        },
    )
}

#[test]
fn terminal_receipt_backpressure_expires_without_dropping_the_pending_frame() -> TestResult {
    worker_terminal_receipt_backpressure_expires_without_dropping_the_pending_frame()
}
pub(in crate::linux) fn worker_terminal_receipt_backpressure_waits_for_the_original_ack_deadline() -> TestResult {
    isolated_with_timeout(
        "linux::tests::terminal_receipt_backpressure_waits_for_the_original_ack_deadline",
        super::ACK_TIMEOUT + Duration::from_secs(3),
        || {
            let mut supervisor = loop_supervisor()?;
            supervisor.phase = super::Phase::Absent;
            supervisor.input_closed = true;
            let event = super::Event::FamilyAbsent {
                execution_id: supervisor.execution.clone(),
                released: false.into(),
                root_exit_code: None,
                root_signal: None,
                timed_out: false.into(),
                canceled: true.into(),
                reaped_count: 0,
                stdout_truncated: false.into(),
                stderr_truncated: false.into(),
            };
            let bytes = super::encode_event(&event)?.len();
            supervisor.writer.queue(event)?;
            refuse_stdout_frame_write(bytes)?;
            let terminal_at = Instant::now();
            supervisor.terminal_at = Some(terminal_at);
            assert_eq!(supervisor.run_loop(), 74);
            assert!(terminal_at.elapsed() >= super::ACK_TIMEOUT);
            assert_eq!(supervisor.writer.pending.len(), 1);
            assert_eq!(supervisor.writer.control_bytes, bytes);
            assert!(!supervisor.writer.closed);
            assert!(!supervisor.acknowledged);
            Ok(())
        },
    )
}

#[test]
fn terminal_receipt_backpressure_waits_for_the_original_ack_deadline() -> TestResult {
    worker_terminal_receipt_backpressure_waits_for_the_original_ack_deadline()
}

pub(in crate::linux) fn worker_refused_external_pidfd_signal_does_not_prevent_held_timeout_cleanup() -> TestResult {
    isolated_process(
        "linux::tests::refused_external_pidfd_signal_does_not_prevent_held_timeout_cleanup",
        || {
            super::probe()?;
            let mut supervisor = loop_supervisor()?;
            let command = super::decode_command(
                br#"{"kind":"init","version":1,"execution_id":"loop-contract","argv":["/bin/true"],"env":{}}"#,
            )?;
            let prepared = super::Prepared::from_command(command)?;
            supervisor.root = Some(super::spawn(&prepared)?);
            let (control_read, control_write) = super::pipe()?;
            assert_eq!(unsafe { libc::dup2(super::AsRawFd::as_raw_fd(&control_write), 1) }, 1);
            supervisor.phase = super::Phase::Held;
            supervisor.input_closed = true;
            supervisor.release_deadline = Some(Instant::now());
            super::state_tests::refuse_signal(libc::SIGTERM, libc::EPERM)?;
            super::EXTERNAL_SIGNAL.store(libc::SIGTERM, super::Ordering::Relaxed);
            assert_eq!(supervisor.run_loop(), 0);
            assert!(matches!(supervisor.phase, super::Phase::Absent));
            assert!(supervisor.canceled);
            assert!(supervisor.timed_out);
            assert!(!supervisor.released);
            assert!(supervisor.root.as_ref().is_some_and(|root| root.reaped));
            assert_eq!(supervisor.reaped, 1);
            assert!(supervisor.cleanup_started.is_some());
            let mut frames = std::io::BufReader::new(std::fs::File::from(control_read));
            assert!(matches!(
                crate::protocol::read_event(&mut frames)?.map(crate::Event::into_wire),
                Some(super::Event::Error {
                    code: super::ErrorCode::IdentityUnknown,
                    uncertain: crate::Certainty::Uncertain,
                    ..
                })
            ));
            assert!(matches!(
                crate::protocol::read_event(&mut frames)?.map(crate::Event::into_wire),
                Some(super::Event::FamilyAbsent {
                    released: crate::ReleaseState::Held,
                    timed_out: crate::TimeoutState::TimedOut,
                    canceled: crate::Cancellation::Canceled,
                    reaped_count: 1,
                    ..
                })
            ));
            Ok(())
        },
    )
}

#[test]
fn refused_external_pidfd_signal_does_not_prevent_held_timeout_cleanup() -> TestResult {
    worker_refused_external_pidfd_signal_does_not_prevent_held_timeout_cleanup()
}

fn refused_probe(name: &str, syscall: libc::c_long) -> TestResult {
    isolated(name, || {
        refuse_syscall(syscall, libc::EPERM)?;
        assert_eq!(super::probe(), Err(super::ErrorCode::KernelPrerequisite));
        Ok(())
    })
}

pub(in crate::linux) fn worker_unavailable_signal_dispositions_refuse_the_native_probe() -> TestResult {
    refused_probe(
        "linux::tests::unavailable_signal_dispositions_refuse_the_native_probe",
        libc::SYS_rt_sigaction,
    )
}

#[test]
fn unavailable_signal_dispositions_refuse_the_native_probe() -> TestResult {
    worker_unavailable_signal_dispositions_refuse_the_native_probe()
}

pub(in crate::linux) fn worker_unavailable_signal_masks_refuse_the_native_probe() -> TestResult {
    refused_probe(
        "linux::tests::unavailable_signal_masks_refuse_the_native_probe",
        libc::SYS_rt_sigprocmask,
    )
}

#[test]
fn unavailable_signal_masks_refuse_the_native_probe() -> TestResult {
    worker_unavailable_signal_masks_refuse_the_native_probe()
}

pub(in crate::linux) fn worker_unavailable_proc_filesystem_metadata_refuses_the_native_probe() -> TestResult {
    refused_probe(
        "linux::tests::unavailable_proc_filesystem_metadata_refuses_the_native_probe",
        libc::SYS_statfs,
    )
}

#[test]
fn unavailable_proc_filesystem_metadata_refuses_the_native_probe() -> TestResult {
    worker_unavailable_proc_filesystem_metadata_refuses_the_native_probe()
}

pub(in crate::linux) fn worker_unavailable_descriptor_isolation_refuses_the_native_probe() -> TestResult {
    refused_probe(
        "linux::tests::unavailable_descriptor_isolation_refuses_the_native_probe",
        libc::SYS_close_range,
    )
}

#[test]
fn unavailable_descriptor_isolation_refuses_the_native_probe() -> TestResult {
    worker_unavailable_descriptor_isolation_refuses_the_native_probe()
}

pub(in crate::linux) fn worker_an_existing_owned_child_prevents_supervisor_activation() -> TestResult {
    isolated_process(
        "linux::tests::an_existing_owned_child_prevents_supervisor_activation",
        || {
            let mut child = Command::new("/bin/sh")
                .args(["-c", "read -r line"])
                .stdin(Stdio::piped())
                .spawn()?;
            let result = super::probe();
            drop(child.stdin.take());
            child.wait()?;
            assert_eq!(result, Err(super::ErrorCode::KernelPrerequisite));
            Ok(())
        },
    )
}

#[test]
fn an_existing_owned_child_prevents_supervisor_activation() -> TestResult {
    worker_an_existing_owned_child_prevents_supervisor_activation()
}

pub(in crate::linux) fn worker_a_nonterminal_kernel_notification_cannot_become_a_root_exit() -> TestResult {
    isolated_process(
        "linux::tests::a_nonterminal_kernel_notification_cannot_become_a_root_exit",
        || {
            let mut child = Command::new("/bin/sh").args(["-c", "kill -STOP $$"]).spawn()?;
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe { libc::waitid(libc::P_PID, child.id(), &mut info, libc::WSTOPPED | libc::WNOWAIT) };
            let killed = child.kill();
            child.wait()?;
            killed?;
            assert_eq!(result, 0);
            assert_eq!(info.si_code, libc::CLD_STOPPED);
            assert!(matches!(
                super::RootStatus::from_siginfo(&info),
                Err(super::ErrorCode::IdentityUnknown)
            ));
            Ok(())
        },
    )
}

#[test]
fn a_nonterminal_kernel_notification_cannot_become_a_root_exit() -> TestResult {
    worker_a_nonterminal_kernel_notification_cannot_become_a_root_exit()
}

pub(in crate::linux) fn worker_initial_control_queue_exhaustion_refuses_start_without_changing_execution_state()
-> TestResult {
    let mut supervisor = loop_supervisor()?;
    let event = super::Event::Status {
        execution_id: supervisor.execution.clone(),
        phase: supervisor.phase,
    };
    while supervisor.writer.queue(event.clone()).is_ok() {}
    let queued = supervisor.writer.pending.len();
    assert_eq!(supervisor.start(super::BTreeMap::new()), 74);
    assert_eq!(supervisor.writer.pending.len(), queued);
    assert!(matches!(supervisor.phase, super::Phase::AwaitingInit));
    assert!(supervisor.root.is_none());
    assert!(!supervisor.released);
    assert!(!supervisor.canceled);
    Ok(())
}

#[test]
fn initial_control_queue_exhaustion_refuses_start_without_changing_execution_state() -> TestResult {
    worker_initial_control_queue_exhaustion_refuses_start_without_changing_execution_state()
}

pub(in crate::linux) fn worker_delayed_retirement_emits_one_cleanup_notice_before_confirmed_absence() -> TestResult {
    isolated_process(
        "linux::tests::delayed_retirement_emits_one_cleanup_notice_before_confirmed_absence",
        || {
            super::probe()?;
            let mut child = Command::new("/bin/sh")
                .args(["-c", "read -r line"])
                .stdin(Stdio::piped())
                .spawn()?;
            let descriptor = super::pidfd(child.id() as i32)?;
            let (read, write) = super::pipe()?;
            assert_eq!(unsafe { libc::dup2(super::AsRawFd::as_raw_fd(&write), 1) }, 1);
            let observer = std::thread::spawn(move || -> Result<(), crate::ProtocolError> {
                let mut frames = std::io::BufReader::new(std::fs::File::from(read));
                assert!(matches!(
                    crate::protocol::read_event(&mut frames)?.map(crate::Event::into_wire),
                    Some(super::Event::Error {
                        code: super::ErrorCode::CleanupPending,
                        uncertain: crate::Certainty::Uncertain,
                        ..
                    })
                ));
                assert_eq!(super::signal_pidfd(&descriptor, libc::SIGKILL), Ok(true));
                assert!(matches!(
                    crate::protocol::read_event(&mut frames)?.map(crate::Event::into_wire),
                    Some(super::Event::FamilyAbsent { reaped_count: 1, .. })
                ));
                Ok(())
            });
            refuse_syscall(libc::SYS_pidfd_send_signal, 0)?;
            let mut supervisor = loop_supervisor()?;
            supervisor.phase = super::Phase::Draining;
            supervisor.canceled = true;
            supervisor.input_closed = true;
            supervisor.cleanup_started = Some(Instant::now() - super::CLEANUP_NOTICE);
            assert_eq!(supervisor.run_loop(), 0);
            observer
                .join()
                .map_err(|_| std::io::Error::other("cleanup observer panicked"))??;
            assert!(supervisor.cleanup_notice);
            assert!(matches!(supervisor.phase, super::Phase::Absent));
            assert_eq!(supervisor.reaped, 1);
            assert!(
                child
                    .try_wait()
                    .is_err_and(|error| error.raw_os_error() == Some(libc::ECHILD))
            );
            Ok(())
        },
    )
}

#[test]
fn delayed_retirement_emits_one_cleanup_notice_before_confirmed_absence() -> TestResult {
    worker_delayed_retirement_emits_one_cleanup_notice_before_confirmed_absence()
}

pub(in crate::linux) fn worker_repeated_wait_refusal_reports_uncertainty_without_claiming_family_absence() -> TestResult
{
    isolated_process(
        "linux::tests::repeated_wait_refusal_reports_uncertainty_without_claiming_family_absence",
        || {
            super::probe()?;
            let command = super::decode_command(
                br#"{"kind":"init","version":1,"execution_id":"loop-contract","argv":["/bin/true"],"env":{}}"#,
            )?;
            let root = super::spawn(&super::Prepared::from_command(command)?)?;
            let pid = root.pid;
            let descriptor = root.pidfd.try_clone()?;
            let (read, write) = super::pipe()?;
            assert_eq!(unsafe { libc::dup2(super::AsRawFd::as_raw_fd(&write), 1) }, 1);
            let _observer = std::thread::spawn(move || {
                let result = (|| -> TestResult {
                    let mut frames = std::io::BufReader::new(std::fs::File::from(read));
                    for _ in 0..2 {
                        assert!(matches!(
                            crate::protocol::read_event(&mut frames)?.map(crate::Event::into_wire),
                            Some(super::Event::Error {
                                code: super::ErrorCode::IdentityUnknown,
                                uncertain: crate::Certainty::Uncertain,
                                ..
                            })
                        ));
                    }
                    let until = Instant::now() + Duration::from_millis(100);
                    while Instant::now() < until {
                        let remaining = until.saturating_duration_since(Instant::now());
                        let mut poll = libc::pollfd {
                            fd: frames.get_ref().as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        if frames.buffer().is_empty() {
                            let result =
                                unsafe { libc::poll(&mut poll, 1, remaining.as_millis().min(i32::MAX as u128) as i32) };
                            if result < 0 {
                                return Err(std::io::Error::last_os_error().into());
                            }
                            if result == 0 {
                                break;
                            }
                        }
                        assert!(
                            matches!(
                                crate::protocol::read_event(&mut frames)?.map(crate::Event::into_wire),
                                Some(super::Event::Error {
                                    code: super::ErrorCode::IdentityUnknown,
                                    uncertain: crate::Certainty::Uncertain,
                                    ..
                                })
                            ),
                            "active wait refusal cannot produce a terminal receipt"
                        );
                    }
                    let _ = super::signal_pidfd(&descriptor, libc::SIGKILL)?;
                    let mut status = 0;
                    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
                    assert!(super::RootStatus::from_raw(status).is_some());
                    Ok(())
                })();
                match result {
                    Ok(()) => std::process::exit(0),
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                }
            });
            refuse_syscall(libc::SYS_waitid, libc::EIO)?;
            let mut supervisor = loop_supervisor()?;
            supervisor.root = Some(root);
            supervisor.phase = super::Phase::Held;
            supervisor.input_closed = true;
            let result = supervisor.run_loop();
            Err(std::io::Error::other(format!("uncertain ownership loop returned {result}")).into())
        },
    )
}

#[test]
fn repeated_wait_refusal_reports_uncertainty_without_claiming_family_absence() -> TestResult {
    worker_repeated_wait_refusal_reports_uncertainty_without_claiming_family_absence()
}

pub(in crate::linux) fn dispatch_worker(name: &str) -> Option<crate::linux::tests::TestResult> {
    match name {
        "linux::tests::child_parser_preserves_below_and_exact_production_limit" => Some({
            worker_child_parser_preserves_below_and_exact_production_limit();
            Ok(())
        }),
        "linux::tests::child_parser_retains_bounded_prefix_above_production_limit" => Some({
            worker_child_parser_retains_bounded_prefix_above_production_limit();
            Ok(())
        }),
        "linux::tests::child_parser_never_authorizes_a_truncated_pid_token" => Some({
            worker_child_parser_never_authorizes_a_truncated_pid_token();
            Ok(())
        }),
        "linux::tests::child_parser_keeps_exhausted_whitespace_boundary_uncertain" => Some({
            worker_child_parser_keeps_exhausted_whitespace_boundary_uncertain();
            Ok(())
        }),
        "linux::tests::child_parser_refuses_malformed_and_nonpositive_complete_ids" => Some({
            worker_child_parser_refuses_malformed_and_nonpositive_complete_ids();
            Ok(())
        }),
        "linux::tests::invalid_descriptors_and_process_handles_preserve_kernel_errors" => {
            Some(worker_invalid_descriptors_and_process_handles_preserve_kernel_errors())
        }
        "linux::tests::syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors" => {
            Some(worker_syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors())
        }
        "linux::tests::interrupted_readiness_is_unknown_without_becoming_a_terminal_error" => {
            Some(worker_interrupted_readiness_is_unknown_without_becoming_a_terminal_error())
        }
        "linux::tests::failed_readiness_preserves_the_io_failure" => {
            Some(worker_failed_readiness_preserves_the_io_failure())
        }
        "linux::tests::signal_callback_retains_the_kernel_signal" => {
            Some(worker_signal_callback_retains_the_kernel_signal())
        }
        "linux::tests::wait_status_distinguishes_signaled_roots_from_stopped_children" => {
            Some(worker_wait_status_distinguishes_signaled_roots_from_stopped_children())
        }
        "linux::tests::a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement" => {
            Some(worker_a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement())
        }
        "linux::tests::closed_control_output_ends_an_absent_loop_without_reopening_cleanup" => {
            Some(worker_closed_control_output_ends_an_absent_loop_without_reopening_cleanup())
        }
        "linux::tests::idle_initialization_deadline_preserves_open_input_and_returns_usage_status" => {
            Some(worker_idle_initialization_deadline_preserves_open_input_and_returns_usage_status())
        }
        "linux::tests::terminal_receipt_backpressure_waits_for_the_original_ack_deadline" => {
            Some(worker_terminal_receipt_backpressure_waits_for_the_original_ack_deadline())
        }
        "linux::tests::terminal_receipt_backpressure_expires_without_dropping_the_pending_frame" => {
            Some(worker_terminal_receipt_backpressure_expires_without_dropping_the_pending_frame())
        }
        "linux::tests::refused_external_pidfd_signal_does_not_prevent_held_timeout_cleanup" => {
            Some(worker_refused_external_pidfd_signal_does_not_prevent_held_timeout_cleanup())
        }
        "linux::tests::unavailable_signal_dispositions_refuse_the_native_probe" => {
            Some(worker_unavailable_signal_dispositions_refuse_the_native_probe())
        }
        "linux::tests::unavailable_signal_masks_refuse_the_native_probe" => {
            Some(worker_unavailable_signal_masks_refuse_the_native_probe())
        }
        "linux::tests::unavailable_proc_filesystem_metadata_refuses_the_native_probe" => {
            Some(worker_unavailable_proc_filesystem_metadata_refuses_the_native_probe())
        }
        "linux::tests::unavailable_descriptor_isolation_refuses_the_native_probe" => {
            Some(worker_unavailable_descriptor_isolation_refuses_the_native_probe())
        }
        "linux::tests::an_existing_owned_child_prevents_supervisor_activation" => {
            Some(worker_an_existing_owned_child_prevents_supervisor_activation())
        }
        "linux::tests::a_nonterminal_kernel_notification_cannot_become_a_root_exit" => {
            Some(worker_a_nonterminal_kernel_notification_cannot_become_a_root_exit())
        }
        "linux::tests::initial_control_queue_exhaustion_refuses_start_without_changing_execution_state" => {
            Some(worker_initial_control_queue_exhaustion_refuses_start_without_changing_execution_state())
        }
        "linux::tests::delayed_retirement_emits_one_cleanup_notice_before_confirmed_absence" => {
            Some(worker_delayed_retirement_emits_one_cleanup_notice_before_confirmed_absence())
        }
        "linux::tests::repeated_wait_refusal_reports_uncertainty_without_claiming_family_absence" => {
            Some(worker_repeated_wait_refusal_reports_uncertainty_without_claiming_family_absence())
        }
        _ => None,
    }
}
