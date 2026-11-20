use super::*;
use crate::CommandWire;
use crate::linux::tests::{TestResult, isolated, refuse_syscall};
use std::cell::RefCell;
use std::io::{Seek, SeekFrom};
use std::process::{Command as Process, Stdio};
use trogon_std::env::ReadEnv;

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

#[cfg(any(
    target_arch = "aarch64",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64"
))]
const DUPLICATE_SYSCALL: libc::c_long = libc::SYS_dup3;
#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv32",
    target_arch = "riscv64",
    target_arch = "loongarch64"
)))]
const DUPLICATE_SYSCALL: libc::c_long = libc::SYS_dup2;

struct ChildCustody {
    children: RefCell<std::fs::File>,
}

impl ChildCustody {
    fn new() -> Result<Self, io::Error> {
        let task = unsafe { libc::syscall(libc::SYS_gettid) } as i32;
        let custody = Self {
            children: RefCell::new(std::fs::File::open(format!("/proc/self/task/{task}/children"))?),
        };
        assert!(custody.children()?.is_empty());
        Ok(custody)
    }

    fn children(&self) -> Result<Vec<i32>, io::Error> {
        let mut file = self.children.borrow_mut();
        file.seek(SeekFrom::Start(0))?;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        text.split_whitespace()
            .map(|pid| pid.parse().map_err(io::Error::other))
            .collect()
    }

    fn settle(&self) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let children = self.children()?;
            if children.is_empty() {
                return Ok(());
            }
            for child in children {
                let mut status = 0;
                let observed = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
                if observed < 0 {
                    return Err(io::Error::last_os_error().into());
                }
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("held child did not settle after setup refusal").into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for ChildCustody {
    fn drop(&mut self) {
        if let Ok(children) = self.children() {
            for child in children {
                unsafe {
                    libc::kill(child, libc::SIGKILL);
                    libc::waitpid(child, std::ptr::null_mut(), 0);
                }
            }
        }
    }
}

fn prepared(marker: Option<&Path>) -> Result<Prepared, Box<dyn std::error::Error>> {
    let argv = marker.map_or_else(
        || vec!["/bin/true".into()],
        |path| {
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf executed > \"$1\"".into(),
                "held-workload".into(),
                path.display().to_string(),
            ]
        },
    );
    let command = CommandWire::Init {
        version: VERSION,
        execution_id: ExecutionId::new("native-setup")?,
        argv,
        env: BTreeMap::new(),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    }
    .try_into()?;
    Ok(Prepared::from_command(command)?)
}

fn install_filter(filter: &mut [libc::sock_filter]) -> TestResult {
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

fn refuse_argument(syscall: libc::c_long, argument: u32, value: u32, error: i32) -> TestResult {
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
            k: 16 + 8 * argument,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: value,
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

fn setup_refusal(name: &str, fault: impl FnOnce() -> TestResult) -> TestResult {
    isolated(name, || {
        let custody = ChildCustody::new()?;
        let workload = prepared(None)?;
        fault()?;
        let result = spawn(&workload);
        custody.settle()?;
        assert!(matches!(result, Err(ErrorCode::SetupFailed)));
        Ok(())
    })
}

#[test]
fn refused_null_input_open_cannot_create_a_held_root() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_null_input_open_cannot_create_a_held_root",
        || {
            refuse_argument(
                libc::SYS_openat,
                2,
                (libc::O_RDONLY | libc::O_CLOEXEC) as u32,
                libc::EMFILE,
            )
        },
    )
}

#[test]
fn refused_child_descriptor_duplication_reports_setup_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_descriptor_duplication_reports_setup_failure",
        || refuse_argument(libc::SYS_fcntl, 1, libc::F_DUPFD_CLOEXEC as u32, libc::EMFILE),
    )
}

#[test]
fn refused_child_standard_descriptor_setup_reports_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_standard_descriptor_setup_reports_failure",
        || refuse_syscall(DUPLICATE_SYSCALL, libc::EPERM),
    )
}

#[test]
fn refused_child_descriptor_closure_reports_setup_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_descriptor_closure_reports_setup_failure",
        || refuse_syscall(libc::SYS_close_range, libc::EPERM),
    )
}

#[test]
fn refused_child_signal_disposition_reports_setup_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_signal_disposition_reports_setup_failure",
        || refuse_syscall(libc::SYS_rt_sigaction, libc::EPERM),
    )
}

#[test]
fn failed_ready_notification_cannot_release_a_workload() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::failed_ready_notification_cannot_release_a_workload",
        || refuse_argument(libc::SYS_write, 0, 4, libc::EPIPE),
    )
}

#[test]
fn failed_pidfd_acquisition_closes_the_held_launch_gate() -> TestResult {
    isolated(
        "linux::native_child::tests::failed_pidfd_acquisition_closes_the_held_launch_gate",
        || {
            let custody = ChildCustody::new()?;
            let workload = prepared(None)?;
            refuse_syscall(libc::SYS_pidfd_open, libc::EPERM)?;
            assert!(matches!(spawn(&workload), Err(ErrorCode::IdentityUnknown)));
            custody.settle()?;
            Ok(())
        },
    )
}

#[test]
fn successful_syscall_without_session_change_is_not_identity_evidence() -> TestResult {
    isolated(
        "linux::native_child::tests::successful_syscall_without_session_change_is_not_identity_evidence",
        || {
            let custody = ChildCustody::new()?;
            let workload = prepared(None)?;
            refuse_syscall(libc::SYS_setsid, 0)?;
            assert!(matches!(spawn(&workload), Err(ErrorCode::IdentityUnknown)));
            custody.settle()?;
            Ok(())
        },
    )
}

fn wait_root(root: &Root) -> Result<RootStatus, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut status = 0;
        let observed = unsafe { libc::waitpid(root.pid, &mut status, libc::WNOHANG) };
        if observed == root.pid {
            return RootStatus::from_raw(status).ok_or_else(|| io::Error::other("terminal child status").into());
        }
        if observed < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("held root did not exit within its bound").into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn invalid_launch_gate_byte_never_executes_the_workload() -> TestResult {
    isolated(
        "linux::native_child::tests::invalid_launch_gate_byte_never_executes_the_workload",
        || {
            let _custody = ChildCustody::new()?;
            let directory = tempfile::tempdir()?;
            let marker = directory.path().join("executed");
            let workload = prepared(Some(&marker))?;
            let root = spawn(&workload)?;
            let gate = root.gate.as_ref().ok_or_else(|| io::Error::other("held gate"))?;
            assert_eq!(unsafe { libc::write(gate.as_raw_fd(), b"X".as_ptr().cast(), 1) }, 1);
            let status = wait_root(&root)?;
            assert_eq!(status.code, Some(125));
            assert_eq!(status.signal, None);
            assert!(!marker.exists());
            Ok(())
        },
    )
}

extern "C" fn interrupt_gate(_: i32) {
    unsafe {
        libc::write(1, b"I".as_ptr().cast(), 1);
    }
}

fn wait_for_gate_read(root: &Root) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(2);
    let syscall = format!("{} 0x3 ", libc::SYS_read);
    loop {
        if std::fs::read_to_string(format!("/proc/{}/syscall", root.pid))?.starts_with(&syscall) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("held root did not block on its launch gate").into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn an_interrupted_gate_read_retries_before_an_explicit_release() -> TestResult {
    isolated(
        "linux::native_child::tests::an_interrupted_gate_read_retries_before_an_explicit_release",
        || {
            let _custody = ChildCustody::new()?;
            unsafe { disposition(libc::SIGUSR1, interrupt_gate as *const () as libc::sighandler_t)? };
            let directory = tempfile::tempdir()?;
            let marker = directory.path().join("executed");
            let root = spawn(&prepared(Some(&marker))?)?;
            wait_for_gate_read(&root)?;
            assert_eq!(signal_pidfd(&root.pidfd, libc::SIGUSR1), Ok(true));
            let output = root.stdout.as_ref().ok_or_else(|| io::Error::other("held output"))?;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let mut byte = [0_u8];
                let count = unsafe { libc::read(output.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
                if count == 1 {
                    assert_eq!(byte, *b"I");
                    break;
                }
                if count < 0 && errno() != libc::EAGAIN {
                    return Err(io::Error::last_os_error().into());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::other("held root did not observe its interrupt").into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            wait_for_gate_read(&root)?;
            assert!(!marker.exists());
            let gate = root.gate.as_ref().ok_or_else(|| io::Error::other("held gate"))?;
            assert_eq!(unsafe { libc::write(gate.as_raw_fd(), b"R".as_ptr().cast(), 1) }, 1);
            let status = wait_root(&root)?;
            assert_eq!(status.code, Some(0));
            assert_eq!(status.signal, None);
            assert_eq!(std::fs::read_to_string(marker)?, "executed");
            Ok(())
        },
    )
}

#[test]
fn absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root() -> TestResult {
    const NAME: &str =
        "linux::native_child::tests::absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root";
    if SystemEnv.var_os("SUPERVISOR_NATIVE_DEADLINE").as_deref() == Some(std::ffi::OsStr::new(NAME)) {
        let custody = ChildCustody::new()?;
        let workload = prepared(None)?;
        refuse_syscall(POLL_SYSCALL, 0)?;
        let start = Instant::now();
        assert!(matches!(spawn(&workload), Err(ErrorCode::SetupFailed)));
        assert!(start.elapsed() >= Duration::from_secs(5));
        custody.settle()?;
        return Ok(());
    }
    let mut child = Process::new(std::env::current_exe()?)
        .args(["--exact", NAME, "--nocapture"])
        .env("SUPERVISOR_NATIVE_DEADLINE", NAME)
        .envs(crate::linux::tests::continuous_profile())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err(io::Error::other("setup deadline test exceeded its parent bound").into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
