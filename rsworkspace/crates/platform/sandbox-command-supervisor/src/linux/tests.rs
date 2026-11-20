use super::parse_children;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use trogon_std::env::ReadEnv;

pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;

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

pub(super) fn isolated(name: &str, body: impl FnOnce() -> TestResult) -> TestResult {
    if super::SystemEnv.var_os("SUPERVISOR_KERNEL_TEST").as_deref() == Some(std::ffi::OsStr::new(name)) {
        return body();
    }
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", name, "--nocapture"])
        .env("SUPERVISOR_KERNEL_TEST", name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(std::io::Error::other("kernel boundary test exceeded its deadline").into());
        }
        std::thread::sleep(Duration::from_millis(5));
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

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_preserves_below_and_exact_production_limit() {
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
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_retains_bounded_prefix_above_production_limit() {
    let snapshot = parse_children(&identifiers(PRODUCTION_LIMIT + 1), false, PRODUCTION_LIMIT)
        .unwrap_or_else(|_| panic!("complete bounded prefix"));
    assert_eq!(snapshot.children.len(), PRODUCTION_LIMIT);
    assert_eq!(snapshot.children.last(), Some(&(PRODUCTION_LIMIT as i32)));
    assert!(snapshot.has_more);
}

#[test]
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_never_authorizes_a_truncated_pid_token() {
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
#[expect(
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn child_parser_keeps_exhausted_whitespace_boundary_uncertain() {
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
fn child_parser_refuses_malformed_and_nonpositive_complete_ids() {
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
fn invalid_descriptors_and_process_handles_preserve_kernel_errors() -> TestResult {
    assert_eq!(super::nonblocking(-1), Err(super::ErrorCode::IoFailure));
    assert!(matches!(super::pidfd(-1), Err(super::ErrorCode::IdentityUnknown)));
    let (read, _) = super::pipe()?;
    assert_eq!(super::signal_pidfd(&read, 0), Err(super::ErrorCode::IdentityUnknown));
    Ok(())
}

#[test]
fn syscall_refusal_does_not_turn_failed_pipe_creation_into_owned_descriptors() -> TestResult {
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
fn interrupted_readiness_is_unknown_without_becoming_a_terminal_error() -> TestResult {
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
fn failed_readiness_preserves_the_io_failure() -> TestResult {
    isolated("linux::tests::failed_readiness_preserves_the_io_failure", || {
        let (read, _) = super::pipe()?;
        refuse_syscall(POLL_SYSCALL, libc::EIO)?;
        assert_eq!(super::readable(&read), Err(super::ErrorCode::IoFailure));
        Ok(())
    })
}

#[test]
fn signal_callback_retains_the_kernel_signal() -> TestResult {
    isolated("linux::tests::signal_callback_retains_the_kernel_signal", || {
        unsafe { super::disposition(libc::SIGUSR1, super::remember_signal as *const () as libc::sighandler_t)? };
        assert_eq!(unsafe { libc::raise(libc::SIGUSR1) }, 0);
        assert_eq!(super::EXTERNAL_SIGNAL.swap(0, super::Ordering::Relaxed), libc::SIGUSR1);
        Ok(())
    })
}

#[test]
fn wait_status_distinguishes_signaled_roots_from_stopped_children() -> TestResult {
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
fn a_reaped_child_keeps_its_pidfd_without_authorizing_a_replacement() -> TestResult {
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
