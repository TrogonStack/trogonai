use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn record(path: &Path) {
    let pid = unsafe { libc::getpid() };
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().collect();
    std::fs::write(path, format!("{pid} {}", fields[19])).unwrap();
    flush_fixture_profile();
}

fn linger() {
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until && !STOP.load(Ordering::Relaxed) {
        unsafe {
            libc::usleep(5_000);
        }
    }
}

fn signals() {
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
}

fn escaped(path: &Path, root_exits: bool, root_handles_term: bool) -> i32 {
    if root_handles_term {
        signals();
    }
    let middle = unsafe { libc::fork() };
    assert!(middle >= 0);
    escaped_after_fork(path, root_exits, middle)
}

fn escaped_after_fork(path: &Path, root_exits: bool, middle: i32) -> i32 {
    if middle == 0 {
        unsafe {
            if libc::setsid() < 0 {
                fixture_exit(91);
            }
            libc::clearenv();
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            let leaf = libc::fork();
            escaped_leaf_after_fork(path, leaf);
        }
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(middle, &mut status, 0) }, middle);
    if root_exits {
        return 7;
    }
    linger();
    0
}

fn escaped_leaf_after_fork(path: &Path, leaf: i32) -> ! {
    if leaf < 0 {
        unsafe { fixture_exit(92) }
    }
    if leaf > 0 {
        unsafe { fixture_exit(0) }
    }
    record(path);
    linger();
    unsafe { fixture_exit(0) }
}

extern "C" fn cloned(pointer: *mut libc::c_void) -> i32 {
    let path = unsafe { &*(pointer.cast::<std::path::PathBuf>()) };
    unsafe {
        libc::setsid();
        libc::clearenv();
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    record(path);
    linger();
    0
}

fn clone_without_notification(path: &Path) -> i32 {
    let path = path.to_path_buf();
    let mut stack = vec![0_u8; 1024 * 1024];
    let top = unsafe { stack.as_mut_ptr().add(stack.len()).cast() };
    let child = unsafe { libc::clone(cloned, top, 0, (&path as *const std::path::PathBuf).cast_mut().cast()) };
    assert!(child > 0);
    linger();
    0
}

fn churn(path: &Path) -> i32 {
    let attempted = std::sync::Arc::new(AtomicBool::new(false));
    let mut gate = [-1; 2];
    assert_eq!(unsafe { libc::pipe2(gate.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let attempted = std::sync::Arc::clone(&attempted);
            std::thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(10);
                while Instant::now() < until {
                    let child = unsafe { libc::fork() };
                    churn_after_fork(child, &attempted);
                }
            })
        })
        .collect();
    let until = Instant::now() + Duration::from_secs(5);
    while !attempted.load(Ordering::Acquire) {
        assert!(Instant::now() < until, "churn worker must attempt its first fork");
        std::thread::sleep(Duration::from_millis(1));
    }
    let keeper = unsafe { libc::fork() };
    assert!(keeper >= 0);
    churn_with_keeper(path, threads, gate, keeper)
}

fn churn_with_keeper(path: &Path, threads: Vec<std::thread::JoinHandle<()>>, gate: [i32; 2], keeper: i32) -> i32 {
    if keeper == 0 {
        unsafe {
            libc::close(gate[1]);
            let mut release = [0_u8];
            let received = libc::read(gate[0], release.as_mut_ptr().cast(), 1);
            libc::_exit(if received == 1 && release[0] == b'R' { 0 } else { 96 });
        }
    }
    unsafe {
        libc::close(gate[0]);
    }
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, keeper, 0) };
    assert!(raw >= 0);
    let identity = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw as i32) };
    let mut poll = libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(&identity),
        events: libc::POLLIN,
        revents: 0,
    };
    assert_eq!(
        unsafe { libc::poll(&mut poll, 1, 0) },
        0,
        "keeper must be alive before churn readiness"
    );
    record(path);
    for thread in threads {
        let _ = thread.join();
    }
    assert_eq!(unsafe { libc::write(gate[1], b"R".as_ptr().cast(), 1) }, 1);
    unsafe {
        libc::close(gate[1]);
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(keeper, &mut status, 0) }, keeper);
    assert_eq!(status, 0);
    0
}

fn churn_after_fork(child: i32, attempted: &AtomicBool) {
    if child == 0 {
        unsafe {
            libc::setsid();
            let delay = libc::timespec {
                tv_sec: 0,
                tv_nsec: 30_000_000,
            };
            libc::nanosleep(&delay, std::ptr::null_mut());
            libc::_exit(0);
        }
    }
    attempted.store(true, Ordering::Release);
    if child > 0 {
        let mut status = 0;
        unsafe {
            libc::waitpid(child, &mut status, 0);
        }
    }
}

fn wait_file(path: &Path) {
    let until = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < until);
        unsafe {
            libc::usleep(1_000);
        }
    }
}

fn overflow(path: &Path) -> i32 {
    let mut gate = [-1; 2];
    assert_eq!(unsafe { libc::pipe2(gate.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    let middle = unsafe { libc::fork() };
    assert!(middle >= 0);
    overflow_after_fork(path, gate, middle)
}

fn overflow_after_fork(path: &Path, gate: [i32; 2], middle: i32) -> i32 {
    if middle == 0 {
        unsafe {
            libc::close(gate[1]);
        }
        record(&path.join("middle"));
        for index in 0..10 {
            let leaf = unsafe { libc::fork() };
            assert!(leaf >= 0);
            overflow_leaf_after_fork(path, gate[0], index, leaf);
        }
        let mut release = [0_u8];
        let received = unsafe { libc::read(gate[0], release.as_mut_ptr().cast(), 1) };
        unsafe {
            fixture_exit(if received == 1 && release[0] == b'R' { 0 } else { 95 });
        }
    }
    unsafe {
        libc::close(gate[0]);
    }
    for index in 0..10 {
        wait_file(&path.join(format!("leaf-{index}")));
    }
    record(&path.join("root"));
    wait_file(&path.join("adopt"));
    assert_eq!(unsafe { libc::write(gate[1], b"R".as_ptr().cast(), 1) }, 1);
    unsafe {
        libc::close(gate[1]);
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(middle, &mut status, 0) }, middle);
    linger();
    0
}

fn overflow_leaf_after_fork(path: &Path, gate: i32, index: usize, leaf: i32) {
    if leaf == 0 {
        unsafe {
            libc::close(gate);
            libc::setsid();
            libc::clearenv();
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        record(&path.join(format!("leaf-{index}")));
        linger();
        unsafe {
            fixture_exit(0);
        }
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
pub fn run() -> i32 {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let [mode, path] = arguments.as_slice() else {
        return 64;
    };
    let path = Path::new(path);
    match mode.as_str() {
        "effect" => {
            record(path);
            std::io::stdout()
                .write_all(b"ordinary stdout\n{\"kind\":\"family_absent\",\"execution_id\":\"contract\"}\n")
                .unwrap();
            std::io::stderr().write_all(b"ordinary stderr\n").unwrap();
            7
        }
        "term" => {
            signals();
            record(path);
            linger();
            0
        }
        "escape" => escaped(path, false, false),
        "escape-root-exits" => escaped(path, true, false),
        "term-tree" => escaped(path, false, true),
        "clone" => clone_without_notification(path),
        "churn" => churn(path),
        "overflow" => overflow(path),
        "flood" => {
            record(path);
            let until = Instant::now() + Duration::from_secs(10);
            let bytes = [b'x'; 8192];
            while Instant::now() < until {
                if std::io::stdout().write_all(&bytes).is_err() || std::io::stderr().write_all(&bytes).is_err() {
                    break;
                }
            }
            0
        }
        _ => 64,
    }
}

#[cfg(coverage)]
unsafe extern "C" {
    fn __llvm_profile_is_continuous_mode_enabled() -> libc::c_int;
    fn __llvm_profile_write_file() -> libc::c_int;
}

fn flush_fixture_profile() {
    #[cfg(coverage)]
    unsafe {
        if __llvm_profile_is_continuous_mode_enabled() == 0 {
            __llvm_profile_write_file();
        }
    }
}

unsafe fn fixture_exit(code: i32) -> ! {
    flush_fixture_profile();
    unsafe { libc::_exit(code) }
}
