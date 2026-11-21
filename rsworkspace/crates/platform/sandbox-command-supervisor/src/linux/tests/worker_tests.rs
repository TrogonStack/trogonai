use super::TestResult;
use std::collections::BTreeMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use trogon_std::env::ReadEnv;

static CUSTODY: Mutex<()> = Mutex::new(());
const LOG_LIMIT: u64 = 8 * 1024 * 1024;

pub(super) fn source_fingerprint() -> String {
    static SOURCES: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src");
    fn files<'a>(directory: &'a include_dir::Dir<'a>, output: &mut Vec<&'a include_dir::File<'a>>) {
        output.extend(directory.files());
        for child in directory.dirs() {
            files(child, output);
        }
    }
    let mut sources = Vec::new();
    files(&SOURCES, &mut sources);
    sources.sort_unstable_by_key(|file| file.path());
    let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
    for file in sources {
        file.path().hash(&mut fingerprint);
        file.contents().hash(&mut fingerprint);
    }
    include_bytes!("../../../Cargo.toml").hash(&mut fingerprint);
    (cfg!(debug_assertions), std::env::consts::ARCH).hash(&mut fingerprint);
    format!("{:016x}", fingerprint.finish())
}

fn children() -> io::Result<Vec<i32>> {
    let mut children = Vec::new();
    for task in std::fs::read_dir("/proc/self/task")? {
        let path = task?.path().join("children");
        let bytes = match std::fs::read_to_string(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for token in bytes.split_whitespace() {
            let pid = token.parse::<i32>().map_err(io::Error::other)?;
            if pid <= 0 || children.len() >= crate::protocol::MAX_CHILDREN {
                return Err(io::Error::other("invalid or excessive kernel-owned child listing"));
            }
            children.push(pid);
        }
    }
    children.sort_unstable();
    children.dedup();
    Ok(children)
}

fn birth(pid: i32) -> io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, fields) = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("missing child stat boundary"))?;
    fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::other("missing child birth identity"))?
        .parse()
        .map_err(io::Error::other)
}

struct Custody {
    previous: i32,
    baseline: BTreeMap<i32, (u64, OwnedFd)>,
    restored: bool,
    _lock: MutexGuard<'static, ()>,
}

impl Custody {
    fn enter() -> TestResult<Self> {
        let lock = CUSTODY
            .lock()
            .map_err(|_| io::Error::other("kernel worker custody poisoned"))?;
        let mut baseline = BTreeMap::new();
        for pid in children()? {
            let descriptor = super::super::pidfd(pid)?;
            baseline.insert(pid, (birth(pid)?, descriptor));
        }
        let mut previous = 0;
        if unsafe { libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut previous) } < 0
            || unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self {
            previous,
            baseline,
            restored: false,
            _lock: lock,
        })
    }

    fn settle(&mut self) -> TestResult<usize> {
        let until = Instant::now() + Duration::from_secs(3);
        let mut retired = 0;
        loop {
            let mut pending = Vec::new();
            for pid in children()? {
                if let Some((expected, _)) = self.baseline.get(&pid) {
                    match birth(pid) {
                        Ok(actual) if actual == *expected => continue,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(error.into()),
                        Ok(_) => {}
                    }
                }
                let descriptor = super::super::pidfd(pid)?;
                pending.push(descriptor);
            }
            if pending.is_empty() {
                if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.previous, 0, 0, 0) } < 0 {
                    return Err(io::Error::last_os_error().into());
                }
                self.restored = true;
                return Ok(retired);
            }
            for descriptor in pending {
                super::super::signal_pidfd(&descriptor, libc::SIGKILL)?;
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let result = unsafe {
                    libc::waitid(
                        libc::P_PIDFD,
                        descriptor.as_raw_fd() as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::__WALL,
                    )
                };
                if result < 0 {
                    return Err(io::Error::last_os_error().into());
                }
                if unsafe { info.si_pid() } > 0 {
                    retired += 1;
                }
            }
            if Instant::now() >= until {
                return Err(io::Error::other("owned worker descendants exceeded cleanup deadline").into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for Custody {
    fn drop(&mut self) {
        if !self.restored
            && let Err(error) = self.settle()
        {
            eprintln!("kernel worker custody could not settle; subreaper remains enabled: {error}");
        }
    }
}

struct Worker {
    process: Child,
    descriptor: Option<OwnedFd>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(descriptor) = &self.descriptor {
            let _ = super::super::signal_pidfd(descriptor, libc::SIGKILL);
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn executable() -> io::Result<PathBuf> {
    if let Some(path) = super::super::SystemEnv.var_os("SUPERVISOR_KERNEL_WORKER") {
        return Ok(path.into());
    }
    let current = std::env::current_exe()?;
    let directory = current
        .parent()
        .ok_or_else(|| io::Error::other("missing test executable directory"))?;
    let directory = if directory.file_name().is_some_and(|name| name == "deps") {
        directory
            .parent()
            .ok_or_else(|| io::Error::other("missing Cargo binary directory"))?
    } else {
        directory
    };
    let worker = directory.join("supervisor-kernel-test-worker");
    if !worker.is_file() {
        return Err(io::Error::other(
            "prebuild supervisor-kernel-test-worker with test-support before focused library tests",
        ));
    }
    Ok(worker)
}

fn log(file: &mut File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(LOG_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LOG_LIMIT {
        return Err(io::Error::other("kernel worker diagnostic output exceeded its bound"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub(super) fn run(name: &str, timeout: Duration) -> TestResult {
    run_observed(name, timeout, |_| Ok(()))
}

pub(super) fn run_observed(
    name: &str,
    timeout: Duration,
    mut observe: impl FnMut(&OwnedFd) -> TestResult,
) -> TestResult {
    run_bound(name, timeout, None, &source_fingerprint(), |descriptor| {
        observe(descriptor)?;
        Ok(Readiness::Ready)
    })
}

pub(super) enum Readiness {
    Pending,
    Ready,
}

pub(super) fn run_after_ready(
    name: &str,
    startup_timeout: Duration,
    timeout: Duration,
    observe: impl FnMut(&OwnedFd) -> TestResult<Readiness>,
) -> TestResult {
    run_bound(name, timeout, Some(startup_timeout), &source_fingerprint(), observe)
}

fn run_bound(
    name: &str,
    timeout: Duration,
    startup_timeout: Option<Duration>,
    fingerprint: &str,
    mut observe: impl FnMut(&OwnedFd) -> TestResult<Readiness>,
) -> TestResult {
    let mut custody = Custody::enter()?;
    let mut errors = tempfile::tempfile()?;
    let mut command = Command::new(executable()?);
    command
        .arg(name)
        .arg(fingerprint)
        .env("SUPERVISOR_KERNEL_TEST", name)
        .envs(super::continuous_profile())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors.try_clone()?);
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: LOG_LIMIT * 2,
                rlim_max: LOG_LIMIT * 2,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut worker = Worker {
        process: command.spawn()?,
        descriptor: None,
    };
    worker.descriptor = Some(super::super::pidfd(worker.process.id() as i32)?);
    let descriptor = worker
        .descriptor
        .as_ref()
        .ok_or_else(|| io::Error::other("missing exact worker identity"))?;
    let mut until = Instant::now() + startup_timeout.unwrap_or(timeout);
    let mut armed = startup_timeout.is_none();
    let timeout_error = || io::Error::new(io::ErrorKind::TimedOut, "kernel worker exceeded its deadline");
    let outcome = loop {
        if Instant::now() >= until {
            break Err(timeout_error());
        }
        let ready = observe(descriptor)?;
        let now = Instant::now();
        if now >= until {
            break Err(timeout_error());
        }
        if !armed && matches!(ready, Readiness::Ready) {
            until = now + timeout;
            armed = true;
        }
        if let Some(status) = worker.process.try_wait()? {
            break Ok(status);
        }
        if Instant::now() >= until {
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "kernel worker exceeded its deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    observe(descriptor)?;
    drop(worker);
    let retired = custody.settle()?;
    let status = outcome?;
    if matches!(status.code(), Some(64 | 65)) {
        return Err(io::Error::other("stale kernel worker; rebuild with: cargo build -p sandbox-command-supervisor --features test-support --bin supervisor-kernel-test-worker").into());
    }
    if !status.success() {
        return Err(io::Error::other(format!("kernel worker failed: {}", log(&mut errors)?)).into());
    }
    if retired != 0 {
        return Err(io::Error::other("successful worker left descendants requiring forced cleanup").into());
    }
    Ok(())
}

#[test]
fn stale_worker_is_rejected_before_the_case_runs() -> TestResult {
    let result = run_bound(
        "linux::native_child::tests::a_retired_child_without_setup_readiness_is_not_a_ready_root",
        Duration::from_secs(5),
        None,
        "stale-source",
        |_| Ok(Readiness::Ready),
    );
    let error = result
        .err()
        .ok_or_else(|| io::Error::other("stale worker executed the case"))?;
    assert!(error.to_string().contains("stale kernel worker"), "{error}");
    Ok(())
}
