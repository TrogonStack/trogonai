use super::{TestResult, worker};
use crate::linux::{pidfd, readable};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct RetainedChild(Option<Child>);

impl Drop for RetainedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn start_time(pid: u32) -> io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or_else(|| io::Error::other("missing process identity"))?
        .parse()
        .map_err(io::Error::other)
}

#[expect(
    clippy::panic,
    reason = "An intentional assertion panic must propagate as a failed worker exit"
)]
fn failing_worker(mode: &str, marker: &Path) -> TestResult {
    let mut command = Command::new("/bin/sleep");
    command
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = RetainedChild(Some(command.spawn()?));
    let process = child
        .0
        .as_ref()
        .ok_or_else(|| io::Error::other("missing escaped worker child"))?;
    let identity = format!("{} {}", process.id(), start_time(process.id())?);
    let pending = marker.with_extension("pending");
    std::fs::write(&pending, identity)?;
    std::fs::rename(pending, marker)?;
    drop(child.0.take());
    if mode == "panic" {
        panic!("intentional assertion failure after creating an escaped child");
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub(super) fn dispatch_worker(name: &str) -> Option<TestResult> {
    match name {
        "linux::tests::containment_tests::worker_assertion_panic_is_failure_and_reaps_escaped_descendants" => {
            return Some(containment("panic"));
        }
        "linux::tests::containment_tests::worker_timeout_reaps_escaped_descendants_and_preserves_sentinel" => {
            return Some(containment("timeout"));
        }
        _ => {}
    }
    let input = name.strip_prefix("custody-case:")?;
    let (mode, marker) = input.split_once(':')?;
    match mode {
        "panic" | "timeout" => Some(failing_worker(mode, Path::new(marker))),
        _ => None,
    }
}

fn containment(mode: &str) -> TestResult {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("escaped");
    let mut sentinel = RetainedChild(Some(Command::new("/bin/sleep").arg("30").spawn()?));
    let sentinel_process = sentinel
        .0
        .as_ref()
        .ok_or_else(|| io::Error::other("missing sentinel"))?;
    let sentinel_fd = pidfd(sentinel_process.id() as i32)?;
    let mut worker_fd: Option<OwnedFd> = None;
    let mut escaped_fd: Option<OwnedFd> = None;
    let name = format!("custody-case:{mode}:{}", marker.display());
    let result = worker::run_observed(&name, Duration::from_millis(200), |descriptor| {
        if worker_fd.is_none() {
            worker_fd = Some(descriptor.try_clone()?);
        }
        if escaped_fd.is_none() && marker.is_file() {
            let text = std::fs::read_to_string(&marker)?;
            let mut fields = text.split_whitespace();
            let pid: u32 = fields
                .next()
                .ok_or_else(|| io::Error::other("missing escaped PID"))?
                .parse()?;
            let birth: u64 = fields
                .next()
                .ok_or_else(|| io::Error::other("missing escaped birth"))?
                .parse()?;
            let retained = pidfd(pid as i32)?;
            assert_eq!(start_time(pid)?, birth);
            escaped_fd = Some(retained);
        }
        Ok(())
    });
    let error = result
        .err()
        .ok_or_else(|| io::Error::other("failing worker was accepted"))?;
    let native = error
        .downcast_ref::<io::Error>()
        .ok_or_else(|| io::Error::other("worker failure lost typed I/O error"))?;
    assert_eq!(
        native.kind(),
        if mode == "timeout" {
            io::ErrorKind::TimedOut
        } else {
            io::ErrorKind::Other
        }
    );
    assert!(readable(
        &worker_fd.ok_or_else(|| io::Error::other("worker was not pinned"))?
    )?);
    assert!(readable(
        &escaped_fd.ok_or_else(|| io::Error::other("escaped child was not pinned"))?
    )?);
    assert!(!readable(&sentinel_fd)?);
    assert!(
        sentinel
            .0
            .as_mut()
            .ok_or_else(|| io::Error::other("missing sentinel"))?
            .try_wait()?
            .is_none()
    );
    Ok(())
}

#[test]
fn worker_assertion_panic_is_failure_and_reaps_escaped_descendants() -> TestResult {
    super::isolated_process(
        "linux::tests::containment_tests::worker_assertion_panic_is_failure_and_reaps_escaped_descendants",
        || containment("panic"),
    )
}

#[test]
fn worker_timeout_reaps_escaped_descendants_and_preserves_sentinel() -> TestResult {
    super::isolated_process(
        "linux::tests::containment_tests::worker_timeout_reaps_escaped_descendants_and_preserves_sentinel",
        || containment("timeout"),
    )
}
