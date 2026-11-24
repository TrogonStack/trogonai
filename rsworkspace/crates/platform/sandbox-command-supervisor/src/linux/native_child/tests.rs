use super::*;
use crate::CommandWire;
use crate::linux::tests::{TestResult, isolated, refuse_syscall};
use std::cell::RefCell;
use std::io::{Seek, SeekFrom};
use std::process::Command as Process;

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
        // `read_to_string` sizes each read from the text it already holds, so a
        // two-digit child PID makes one of them five bytes long, which the
        // readiness faults refuse for every read in the worker.
        let mut text = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            match file.read(&mut chunk)? {
                0 => break,
                read => text.extend_from_slice(&chunk[..read]),
            }
        }
        String::from_utf8(text)
            .map_err(io::Error::other)?
            .split_whitespace()
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
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("executed");
        let workload = prepared(Some(&marker))?;
        fault()?;
        let mut result = spawn(&workload);
        if let Ok(root) = &mut result {
            root.gate.take();
        }
        custody.settle()?;
        assert!(matches!(result, Err(error) if error.code() == ErrorCode::SetupFailed));
        assert!(!marker.exists());
        Ok(())
    })
}

pub(in crate::linux) fn worker_refused_null_input_open_cannot_create_a_held_root() -> TestResult {
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
fn refused_null_input_open_cannot_create_a_held_root() -> TestResult {
    worker_refused_null_input_open_cannot_create_a_held_root()
}

pub(in crate::linux) fn worker_refused_child_descriptor_duplication_reports_setup_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_descriptor_duplication_reports_setup_failure",
        || refuse_argument(libc::SYS_fcntl, 1, libc::F_DUPFD_CLOEXEC as u32, libc::EMFILE),
    )
}

#[test]
fn refused_child_descriptor_duplication_reports_setup_failure() -> TestResult {
    worker_refused_child_descriptor_duplication_reports_setup_failure()
}

pub(in crate::linux) fn worker_refused_child_standard_descriptor_setup_reports_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_standard_descriptor_setup_reports_failure",
        || refuse_syscall(DUPLICATE_SYSCALL, libc::EPERM),
    )
}

#[test]
fn refused_child_standard_descriptor_setup_reports_failure() -> TestResult {
    worker_refused_child_standard_descriptor_setup_reports_failure()
}

pub(in crate::linux) fn worker_refused_child_descriptor_closure_reports_setup_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_descriptor_closure_reports_setup_failure",
        || refuse_syscall(libc::SYS_close_range, libc::EPERM),
    )
}

#[test]
fn refused_child_descriptor_closure_reports_setup_failure() -> TestResult {
    worker_refused_child_descriptor_closure_reports_setup_failure()
}

pub(in crate::linux) fn worker_refused_child_signal_disposition_reports_setup_failure() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_child_signal_disposition_reports_setup_failure",
        || refuse_syscall(libc::SYS_rt_sigaction, libc::EPERM),
    )
}

#[test]
fn refused_child_signal_disposition_reports_setup_failure() -> TestResult {
    worker_refused_child_signal_disposition_reports_setup_failure()
}

pub(in crate::linux) fn worker_failed_ready_notification_cannot_release_a_workload() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::failed_ready_notification_cannot_release_a_workload",
        || refuse_argument(libc::SYS_write, 0, 4, libc::EPIPE),
    )
}

#[test]
fn failed_ready_notification_cannot_release_a_workload() -> TestResult {
    worker_failed_ready_notification_cannot_release_a_workload()
}

pub(in crate::linux) fn worker_failed_pidfd_acquisition_closes_the_held_launch_gate() -> TestResult {
    isolated(
        "linux::native_child::tests::failed_pidfd_acquisition_closes_the_held_launch_gate",
        || {
            let custody = ChildCustody::new()?;
            let directory = tempfile::tempdir()?;
            let marker = directory.path().join("executed");
            let workload = prepared(Some(&marker))?;
            refuse_syscall(libc::SYS_pidfd_open, libc::EPERM)?;
            assert!(matches!(spawn(&workload), Err(error) if error.code() == ErrorCode::IdentityUnknown));
            custody.settle()?;
            assert!(!marker.exists());
            Ok(())
        },
    )
}

#[test]
fn failed_pidfd_acquisition_closes_the_held_launch_gate() -> TestResult {
    worker_failed_pidfd_acquisition_closes_the_held_launch_gate()
}

pub(in crate::linux) fn worker_successful_syscall_without_session_change_is_not_identity_evidence() -> TestResult {
    isolated(
        "linux::native_child::tests::successful_syscall_without_session_change_is_not_identity_evidence",
        || {
            let custody = ChildCustody::new()?;
            let directory = tempfile::tempdir()?;
            let marker = directory.path().join("executed");
            let workload = prepared(Some(&marker))?;
            refuse_syscall(libc::SYS_setsid, 0)?;
            assert!(matches!(spawn(&workload), Err(error) if error.code() == ErrorCode::IdentityUnknown));
            custody.settle()?;
            assert!(!marker.exists());
            Ok(())
        },
    )
}

#[test]
fn successful_syscall_without_session_change_is_not_identity_evidence() -> TestResult {
    worker_successful_syscall_without_session_change_is_not_identity_evidence()
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

pub(in crate::linux) fn worker_invalid_launch_gate_byte_never_executes_the_workload() -> TestResult {
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

#[test]
fn invalid_launch_gate_byte_never_executes_the_workload() -> TestResult {
    worker_invalid_launch_gate_byte_never_executes_the_workload()
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

pub(in crate::linux) fn worker_an_interrupted_gate_read_retries_before_an_explicit_release() -> TestResult {
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
fn an_interrupted_gate_read_retries_before_an_explicit_release() -> TestResult {
    worker_an_interrupted_gate_read_retries_before_an_explicit_release()
}

fn deadline_refusal(name: &str, fault: impl FnOnce() -> TestResult) -> TestResult {
    crate::linux::tests::isolated_with_timeout(name, Duration::from_secs(10), || {
        let custody = ChildCustody::new()?;
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("executed");
        let workload = prepared(Some(&marker))?;
        fault()?;
        let start = Instant::now();
        assert!(matches!(spawn(&workload), Err(error) if error.code() == ErrorCode::SetupFailed));
        assert!(start.elapsed() >= Duration::from_secs(5));
        custody.settle()?;
        assert!(!marker.exists());
        Ok(())
    })
}

pub(in crate::linux) fn worker_absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root() -> TestResult
{
    deadline_refusal(
        "linux::native_child::tests::absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root",
        || refuse_syscall(POLL_SYSCALL, 0),
    )
}

#[test]
fn absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root() -> TestResult {
    worker_absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root()
}

pub(in crate::linux) fn worker_interrupted_readiness_retries_within_the_original_deadline() -> TestResult {
    deadline_refusal(
        "linux::native_child::tests::interrupted_readiness_retries_within_the_original_deadline",
        || refuse_argument(libc::SYS_read, 2, 5, libc::EINTR),
    )
}

#[test]
fn interrupted_readiness_retries_within_the_original_deadline() -> TestResult {
    worker_interrupted_readiness_retries_within_the_original_deadline()
}

fn refuse_close_on_exec(descriptor: RawFd) -> TestResult {
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
            k: libc::SYS_fcntl as u32,
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
            k: descriptor as u32,
        },
        libc::sock_filter {
            code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
            jt: 0,
            jf: 0,
            k: 24,
        },
        libc::sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: libc::F_SETFD as u32,
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
    install_filter(&mut filter)
}

pub(in crate::linux) fn worker_refused_gate_close_on_exec_cannot_report_ready() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_gate_close_on_exec_cannot_report_ready",
        || refuse_close_on_exec(3),
    )
}

#[test]
fn refused_gate_close_on_exec_cannot_report_ready() -> TestResult {
    worker_refused_gate_close_on_exec_cannot_report_ready()
}

pub(in crate::linux) fn worker_refused_setup_close_on_exec_cannot_report_ready() -> TestResult {
    setup_refusal(
        "linux::native_child::tests::refused_setup_close_on_exec_cannot_report_ready",
        || refuse_close_on_exec(4),
    )
}

#[test]
fn refused_setup_close_on_exec_cannot_report_ready() -> TestResult {
    worker_refused_setup_close_on_exec_cannot_report_ready()
}

fn setup_error_source(name: &str, expected_errno: i32, fault: impl FnOnce() -> TestResult) -> TestResult {
    isolated(name, || {
        let custody = ChildCustody::new()?;
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("executed");
        let mut workload = prepared(Some(&marker))?;
        workload.cwd = Some(CString::new(
            directory.path().join("absent").to_string_lossy().as_bytes(),
        )?);
        fault()?;
        let error = match spawn(&workload) {
            Err(error) => error,
            Ok(mut root) => {
                root.gate.take();
                return Err(io::Error::other("invalid setup reported readiness").into());
            }
        };
        custody.settle()?;
        let source = std::error::Error::source(&error).and_then(|source| source.downcast_ref::<io::Error>());
        assert_eq!(source.and_then(io::Error::raw_os_error), Some(expected_errno));
        assert!(!marker.exists());
        Ok(())
    })
}

pub(in crate::linux) fn worker_missing_working_directory_preserves_native_setup_errno() -> TestResult {
    setup_error_source(
        "linux::native_child::tests::missing_working_directory_preserves_native_setup_errno",
        libc::ENOENT,
        || Ok(()),
    )
}

#[test]
fn missing_working_directory_preserves_native_setup_errno() -> TestResult {
    worker_missing_working_directory_preserves_native_setup_errno()
}

pub(in crate::linux) fn worker_refused_working_directory_preserves_native_setup_errno() -> TestResult {
    setup_error_source(
        "linux::native_child::tests::refused_working_directory_preserves_native_setup_errno",
        libc::EPERM,
        || refuse_syscall(libc::SYS_chdir, libc::EPERM),
    )
}

#[test]
fn refused_working_directory_preserves_native_setup_errno() -> TestResult {
    worker_refused_working_directory_preserves_native_setup_errno()
}

pub(in crate::linux) fn worker_malformed_or_incomplete_setup_records_never_report_readiness() -> TestResult {
    let current = pidfd(unsafe { libc::getpid() })?;
    for packet in [&[][..], &[1][..], &[2][..], &[1, 7, 0][..]] {
        let (read, write) = pipe()?;
        if !packet.is_empty() {
            assert_eq!(
                unsafe { libc::write(write.as_raw_fd(), packet.as_ptr().cast(), packet.len()) },
                packet.len() as isize
            );
        }
        drop(write);
        assert!(matches!(await_readiness(&read, &current), Err(error) if error.code() == ErrorCode::SetupFailed));
    }
    Ok(())
}

#[test]
fn malformed_or_incomplete_setup_records_never_report_readiness() -> TestResult {
    worker_malformed_or_incomplete_setup_records_never_report_readiness()
}

pub(in crate::linux) fn worker_failed_readiness_read_preserves_the_native_io_error() -> TestResult {
    setup_error_source(
        "linux::native_child::tests::failed_readiness_read_preserves_the_native_io_error",
        libc::EIO,
        || refuse_argument(libc::SYS_read, 2, 5, libc::EIO),
    )
}

#[test]
fn failed_readiness_read_preserves_the_native_io_error() -> TestResult {
    worker_failed_readiness_read_preserves_the_native_io_error()
}

pub(in crate::linux) fn worker_a_retired_child_without_setup_readiness_is_not_a_ready_root() -> TestResult {
    isolated(
        "linux::native_child::tests::a_retired_child_without_setup_readiness_is_not_a_ready_root",
        || {
            let custody = ChildCustody::new()?;
            let mut child = Process::new("/bin/true").spawn()?;
            let descriptor = pidfd(child.id() as i32)?;
            assert!(child.wait()?.success());
            let (setup, _writer) = pipe()?;
            assert!(
                matches!(await_readiness(&setup, &descriptor), Err(error) if error.code() == ErrorCode::SetupFailed)
            );
            custody.settle()?;
            Ok(())
        },
    )
}

#[test]
fn a_retired_child_without_setup_readiness_is_not_a_ready_root() -> TestResult {
    worker_a_retired_child_without_setup_readiness_is_not_a_ready_root()
}

pub(in crate::linux) fn dispatch_worker(name: &str) -> Option<crate::linux::tests::TestResult> {
    match name {
        "linux::native_child::tests::refused_null_input_open_cannot_create_a_held_root" => {
            Some(worker_refused_null_input_open_cannot_create_a_held_root())
        }
        "linux::native_child::tests::refused_child_descriptor_duplication_reports_setup_failure" => {
            Some(worker_refused_child_descriptor_duplication_reports_setup_failure())
        }
        "linux::native_child::tests::refused_child_standard_descriptor_setup_reports_failure" => {
            Some(worker_refused_child_standard_descriptor_setup_reports_failure())
        }
        "linux::native_child::tests::refused_child_descriptor_closure_reports_setup_failure" => {
            Some(worker_refused_child_descriptor_closure_reports_setup_failure())
        }
        "linux::native_child::tests::refused_child_signal_disposition_reports_setup_failure" => {
            Some(worker_refused_child_signal_disposition_reports_setup_failure())
        }
        "linux::native_child::tests::failed_ready_notification_cannot_release_a_workload" => {
            Some(worker_failed_ready_notification_cannot_release_a_workload())
        }
        "linux::native_child::tests::failed_pidfd_acquisition_closes_the_held_launch_gate" => {
            Some(worker_failed_pidfd_acquisition_closes_the_held_launch_gate())
        }
        "linux::native_child::tests::successful_syscall_without_session_change_is_not_identity_evidence" => {
            Some(worker_successful_syscall_without_session_change_is_not_identity_evidence())
        }
        "linux::native_child::tests::invalid_launch_gate_byte_never_executes_the_workload" => {
            Some(worker_invalid_launch_gate_byte_never_executes_the_workload())
        }
        "linux::native_child::tests::an_interrupted_gate_read_retries_before_an_explicit_release" => {
            Some(worker_an_interrupted_gate_read_retries_before_an_explicit_release())
        }
        "linux::native_child::tests::absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root" => {
            Some(worker_absent_setup_readiness_reaches_the_original_deadline_and_retires_the_root())
        }
        "linux::native_child::tests::interrupted_readiness_retries_within_the_original_deadline" => {
            Some(worker_interrupted_readiness_retries_within_the_original_deadline())
        }
        "linux::native_child::tests::refused_gate_close_on_exec_cannot_report_ready" => {
            Some(worker_refused_gate_close_on_exec_cannot_report_ready())
        }
        "linux::native_child::tests::refused_setup_close_on_exec_cannot_report_ready" => {
            Some(worker_refused_setup_close_on_exec_cannot_report_ready())
        }
        "linux::native_child::tests::missing_working_directory_preserves_native_setup_errno" => {
            Some(worker_missing_working_directory_preserves_native_setup_errno())
        }
        "linux::native_child::tests::refused_working_directory_preserves_native_setup_errno" => {
            Some(worker_refused_working_directory_preserves_native_setup_errno())
        }
        "linux::native_child::tests::malformed_or_incomplete_setup_records_never_report_readiness" => {
            Some(worker_malformed_or_incomplete_setup_records_never_report_readiness())
        }
        "linux::native_child::tests::failed_readiness_read_preserves_the_native_io_error" => {
            Some(worker_failed_readiness_read_preserves_the_native_io_error())
        }
        "linux::native_child::tests::a_retired_child_without_setup_readiness_is_not_a_ready_root" => {
            Some(worker_a_retired_child_without_setup_readiness_is_not_a_ready_root())
        }
        _ => None,
    }
}
