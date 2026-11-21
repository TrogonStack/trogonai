mod retirement_observer;

use super::*;
use crate::linux::tests::{TestResult, isolated, refuse_syscall};
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command};

struct ChildGuard(Child);

impl ChildGuard {
    fn start() -> io::Result<Self> {
        Command::new("/bin/sleep").arg("30").spawn().map(Self)
    }

    fn target(&self) -> Result<Target, ErrorCode> {
        let pid = self.0.id() as i32;
        Ok(Target {
            pid,
            descriptor: pidfd(pid)?,
        })
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn own() -> Result<Target, ErrorCode> {
    let pid = unsafe { libc::getpid() };
    Ok(Target {
        pid,
        descriptor: pidfd(pid)?,
    })
}

fn snapshot(capacity: usize, remaining: usize) -> Snapshot {
    Snapshot {
        targets: Vec::new(),
        seen: BTreeMap::new(),
        remaining,
        capacity,
    }
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

macro_rules! cases {
    ($($name:ident => $body:block),+ $(,)?) => {
        $(
            #[cfg_attr(test, test)]
            fn $name() -> TestResult {
                isolated(concat!("linux::signal_dispatch::tests::", stringify!($name)), || $body)
            }
        )+
        pub(in crate::linux) fn dispatch_worker(name: &str) -> Option<TestResult> {
            match name {
                $(concat!("linux::signal_dispatch::tests::", stringify!($name)) => Some($name()),)+
                _ => None,
            }
        }
    };
}

cases! {
    process_exit_and_reap_revoke_liveness => {
        let mut child = ChildGuard::start()?;
        let target = child.target()?;
        assert!(alive(&target.descriptor)?);
        child.0.kill()?;
        child.0.wait()?;
        assert!(!alive(&target.descriptor)?);
        let mut poll = libc::pollfd { fd: target.descriptor.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 1);
        assert_ne!(poll.revents & libc::POLLHUP, 0);
        Ok(())
    },
    an_interrupted_liveness_check_cannot_grant_authority => {
        let target = own()?;
        refuse_syscall(POLL_SYSCALL, libc::EINTR)?;
        assert_eq!(alive(&target.descriptor), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    a_failed_liveness_check_cannot_grant_authority => {
        let target = own()?;
        refuse_syscall(POLL_SYSCALL, libc::EIO)?;
        assert_eq!(alive(&target.descriptor), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    unexpected_poll_events_do_not_count_as_live => {
        let (read, write) = crate::linux::pipe()?;
        drop(read);
        assert_eq!(alive(&write), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    duplicate_relations_retain_one_identity => {
        let child = ChildGuard::start()?;
        let parent = own()?;
        let mut snapshot = snapshot(1, 2);
        snapshot.retain(&parent, vec![child.0.id() as i32, child.0.id() as i32])?;
        assert_eq!(snapshot.targets.len(), 1);
        assert_eq!(snapshot.remaining, 0);
        assert!(alive(&snapshot.targets[0].descriptor)?);
        Ok(())
    },
    a_retired_duplicate_cannot_authorize_a_reused_pid => {
        let mut child = ChildGuard::start()?;
        let parent = own()?;
        let mut snapshot = snapshot(1, 2);
        snapshot.retain(&parent, vec![child.0.id() as i32])?;
        child.0.kill()?;
        child.0.wait()?;
        assert_eq!(snapshot.retain(&parent, vec![child.0.id() as i32]), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    mismatched_parent_relations_cannot_authorize_a_process => {
        let child = ChildGuard::start()?;
        let mut parent = own()?;
        parent.pid += 1;
        let mut snapshot = snapshot(1, 1);
        assert_eq!(snapshot.retain(&parent, vec![child.0.id() as i32]), Err(ErrorCode::IdentityUnknown));
        assert!(snapshot.targets.is_empty());
        assert!(alive(&child.target()?.descriptor)?);
        Ok(())
    },
    an_exited_parent_cannot_authorize_a_new_relation => {
        let mut parent = ChildGuard::start()?;
        let pinned_parent = parent.target()?;
        parent.0.kill()?;
        parent.0.wait()?;
        let child = ChildGuard::start()?;
        let mut snapshot = snapshot(1, 1);
        assert_eq!(snapshot.retain(&pinned_parent, vec![child.0.id() as i32]), Err(ErrorCode::IdentityUnknown));
        assert!(snapshot.targets.is_empty());
        Ok(())
    },
    an_unreaped_exited_candidate_is_not_retained => {
        let mut child = ChildGuard::start()?;
        let identity = child.target()?;
        let parent = own()?;
        child.0.kill()?;
        let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe {
            libc::waitid(libc::P_PID, child.0.id(), &mut status, libc::WEXITED | libc::WNOWAIT)
        }, 0);
        assert!(!alive(&identity.descriptor)?);
        let mut snapshot = snapshot(1, 1);
        snapshot.retain(&parent, vec![child.0.id() as i32])?;
        assert!(snapshot.targets.is_empty() && snapshot.seen.is_empty());
        assert_eq!(child.0.wait()?.signal(), Some(libc::SIGKILL));
        Ok(())
    },
    a_parent_that_exits_after_its_task_walk_is_not_trusted => {
        let mut child = ChildGuard::start()?;
        let parent = own()?;
        let mut snapshot = snapshot(1, 4);
        snapshot.retain(&parent, vec![child.0.id() as i32])?;
        let receipt = retirement_observer::install(child.0.id() as i32, child.target()?.descriptor)?;
        assert_eq!(snapshot.descend(0), Err(ErrorCode::IdentityUnknown));
        let receipt = receipt.recv_timeout(std::time::Duration::from_secs(2))??;
        assert_eq!(receipt.child_lists, 1);
        assert_eq!(child.0.wait()?.signal(), Some(libc::SIGKILL));
        Ok(())
    },
    a_target_that_exits_after_retention_cannot_be_traversed => {
        let mut child = ChildGuard::start()?;
        let parent = own()?;
        let mut snapshot = snapshot(1, 4);
        snapshot.retain(&parent, vec![child.0.id() as i32])?;
        child.0.kill()?;
        child.0.wait()?;
        assert_eq!(snapshot.descend(0), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    a_missing_task_directory_cannot_complete_a_snapshot => {
        let child = ChildGuard::start()?;
        let parent = own()?;
        let mut snapshot = snapshot(1, 4);
        snapshot.retain(&parent, vec![child.0.id() as i32])?;
        snapshot.targets[0].pid = i32::MAX;
        assert_eq!(snapshot.descend(0), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    a_task_visit_that_exhausts_the_budget_cannot_complete_a_snapshot => {
        let child = ChildGuard::start()?;
        let parent = own()?;
        let mut snapshot = snapshot(1, 1);
        snapshot.retain(&parent, vec![child.0.id() as i32])?;
        assert_eq!(snapshot.descend(0), Err(ErrorCode::ProcessLimit));
        Ok(())
    },
    relation_and_process_bounds_fail_before_delivery => {
        let child = ChildGuard::start()?;
        let parent = own()?;
        let mut exhausted = snapshot(1, 0);
        assert_eq!(exhausted.retain(&parent, vec![child.0.id() as i32]), Err(ErrorCode::ProcessLimit));
        let mut full = snapshot(0, 1);
        assert_eq!(full.retain(&parent, vec![child.0.id() as i32]), Err(ErrorCode::ProcessLimit));
        assert!(exhausted.targets.is_empty() && full.targets.is_empty());
        assert!(alive(&child.target()?.descriptor)?);
        Ok(())
    },
    vanished_procfs_entries_fail_closed => {
        let mut child = ChildGuard::start()?;
        let pid = child.0.id() as i32;
        child.0.kill()?;
        child.0.wait()?;
        assert_eq!(parent_pid(pid), Err(ErrorCode::IdentityUnknown));
        let parent = own()?;
        let mut snapshot = snapshot(1, 1);
        assert_eq!(snapshot.retain(&parent, vec![pid]), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    parent_records_require_a_complete_numeric_relation => {
        assert_eq!(parse_parent(b"123 (name) S 456 0"), Ok(456));
        assert_eq!(parse_parent(b"123 (name) S"), Err(ErrorCode::IdentityUnknown));
        assert_eq!(parse_parent(b"123 name S 456"), Err(ErrorCode::IdentityUnknown));
        assert_eq!(parse_parent(b"123 (name) S invalid"), Err(ErrorCode::IdentityUnknown));
        assert_eq!(parse_parent(b"123 (name) S \xff"), Err(ErrorCode::IdentityUnknown));
        assert_eq!(parse_parent(b"123 (name with ) and \xff) S 456"), Ok(456));
        Ok(())
    },
    bounded_procfs_reads_reject_incomplete_or_oversized_lists => {
        let directory = tempfile::tempdir()?;
        assert_eq!(read_bounded(directory.path(), 16), Err(ErrorCode::IdentityUnknown));
        assert_eq!(read_bounded(&directory.path().join("missing"), 16), Err(ErrorCode::IdentityUnknown));
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(file.path(), b"12 ")?;
        assert_eq!(children(file.path(), 0), Err(ErrorCode::ProcessLimit));
        assert_eq!(read_bounded(file.path(), 3), Err(ErrorCode::ProcessLimit));
        std::fs::write(file.path(), b"12")?;
        assert_eq!(children(file.path(), 4), Err(ErrorCode::ProcessLimit));
        std::fs::write(file.path(), b"invalid ")?;
        assert_eq!(children(file.path(), 4), Err(ErrorCode::IdentityUnknown));
        Ok(())
    },
    one_refused_target_does_not_skip_other_targets => {
        let mut first = ChildGuard::start()?;
        let mut second = ChildGuard::start()?;
        let first_target = first.target()?;
        let second_target = second.target()?;
        let deny_fd = first_target.descriptor.as_raw_fd() as u32;
        let instruction = |code: u32, jt, jf, k| libc::sock_filter { code: code as u16, jt, jf, k };
        let mut filter = [
            instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 0),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 5, libc::SYS_pidfd_send_signal as u32),
            instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, std::mem::offset_of!(libc::seccomp_data, args) as u32),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 3, deny_fd),
            instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, (std::mem::offset_of!(libc::seccomp_data, args) + std::mem::size_of::<u64>()) as u32),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 1, libc::SIGTERM as u32),
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, libc::SECCOMP_RET_ALLOW),
        ];
        let program = libc::sock_fprog { len: filter.len() as u16, filter: filter.as_mut_ptr() };
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0
            || unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        assert_eq!(dispatch(vec![first_target, second_target], libc::SIGTERM), Err(ErrorCode::IdentityUnknown));
        assert!(first.0.try_wait()?.is_none());
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = second.0.try_wait()? {
                            assert_eq!(status.signal(), Some(libc::SIGTERM));
                break;
            }
            assert!(std::time::Instant::now() < until, "second target must receive its signal despite first refusal");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(())
    },
}
