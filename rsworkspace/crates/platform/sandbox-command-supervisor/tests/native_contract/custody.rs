use super::*;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io;
use std::rc::{Rc, Weak};
use std::sync::{Mutex, MutexGuard};

static CUSTODY: Mutex<()> = Mutex::new(());

thread_local! {
    static CURRENT: RefCell<Weak<State>> = const { RefCell::new(Weak::new()) };
}

struct State {
    previous: i32,
    baseline: BTreeSet<(u32, u64)>,
    _lock: MutexGuard<'static, ()>,
}

pub(super) struct TestCustody {
    _state: Rc<State>,
}

impl TestCustody {
    #[expect(
        clippy::panic,
        reason = "A failed fixture custody baseline must terminate the native contract test"
    )]
    pub(super) fn enter() -> Self {
        CURRENT.with(|current| {
            if let Some(state) = current.borrow().upgrade() {
                return Self { _state: state };
            }
            let lock = CUSTODY.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let baseline = children().unwrap_or_else(|error| panic!("child custody baseline: {error}"));
            let mut previous = 0_i32;
            assert_eq!(unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut previous) }, 0);
            assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) }, 0);
            let state = Rc::new(State {
                previous,
                baseline,
                _lock: lock,
            });
            *current.borrow_mut() = Rc::downgrade(&state);
            Self { _state: state }
        })
    }

    pub(super) fn unavailable_to_another_thread() -> bool {
        matches!(CUSTODY.try_lock(), Err(std::sync::TryLockError::WouldBlock))
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let complete = settle(&self.baseline);
        let restored = complete && unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.previous, 0, 0, 0) } == 0;
        if !complete || !restored {
            eprintln!("native fixture custody restoration incomplete");
            assert!(
                std::thread::panicking(),
                "native fixture custody restoration must complete"
            );
        }
    }
}

pub(super) struct FamilyCustody {
    baseline: BTreeSet<(u32, u64)>,
    _scope: TestCustody,
}

impl FamilyCustody {
    #[expect(
        clippy::panic,
        reason = "A failed fixture custody baseline must terminate the native contract test"
    )]
    pub(super) fn enter() -> Self {
        let scope = TestCustody::enter();
        let baseline = children().unwrap_or_else(|error| panic!("family custody baseline: {error}"));
        Self {
            baseline,
            _scope: scope,
        }
    }

    pub(super) fn settle(&self) -> bool {
        settle(&self.baseline)
    }
}

fn start_time(pid: u32) -> io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
}

fn children() -> io::Result<BTreeSet<(u32, u64)>> {
    let mut children = BTreeSet::new();
    for task in std::fs::read_dir("/proc/self/task")? {
        let task = task?;
        let list = match std::fs::read_to_string(task.path().join("children")) {
            Ok(list) => list,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for value in list.split_whitespace() {
            let pid = value.parse().map_err(io::Error::other)?;
            match start_time(pid) {
                Ok(start) => {
                    children.insert((pid, start));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(children)
}

fn settle(baseline: &BTreeSet<(u32, u64)>) -> bool {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        let observed = match children() {
            Ok(children) => children,
            Err(_) => return false,
        };
        let owned: Vec<_> = observed.difference(baseline).copied().collect();
        if owned.is_empty() {
            return true;
        }
        for (pid, start) in owned {
            let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as i32, 0) };
            if raw < 0 {
                continue;
            }
            let descriptor = unsafe { OwnedFd::from_raw_fd(raw as i32) };
            if start_time(pid).ok() != Some(start) {
                continue;
            }
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    descriptor.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
                let mut info: libc::siginfo_t = std::mem::zeroed();
                libc::waitid(
                    libc::P_PIDFD,
                    descriptor.as_raw_fd() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::__WALL,
                );
            }
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
