use super::*;
use std::io;

fn exited(descriptor: &OwnedFd) -> bool {
    let mut poll = libc::pollfd {
        fd: descriptor.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    unsafe { libc::poll(&mut poll, 1, 0) == 1 && poll.revents & libc::POLLIN != 0 }
}

fn reaped(descriptor: &OwnedFd) -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    (unsafe {
        libc::waitid(
            libc::P_PIDFD,
            descriptor.as_raw_fd() as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT | libc::__WALL,
        )
    }) < 0
        && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
}

struct PinnedCleanup(Vec<OwnedFd>);

impl Drop for PinnedCleanup {
    fn drop(&mut self) {
        for descriptor in &self.0 {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    descriptor.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
        let until = Instant::now() + Duration::from_secs(3);
        for descriptor in &self.0 {
            loop {
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let result = unsafe {
                    libc::waitid(
                        libc::P_PIDFD,
                        descriptor.as_raw_fd() as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::__WALL,
                    )
                };
                if result < 0 || unsafe { info.si_pid() } > 0 || Instant::now() >= until {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

#[test]
fn forced_harness_teardown_reaps_escaped_family_without_touching_sentinel() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("escaped");
    let mut sentinel = Sentinel::start();
    let mut helper = Harness::start();
    let root = pin(helper.initialize("escape", &marker, None, None));
    helper.release();
    let escaped = pin(recorded(&marker));
    let cleanup = PinnedCleanup(vec![root, escaped]);
    assert!(cleanup.0.iter().all(|descriptor| !exited(descriptor)));
    assert_eq!(unsafe { libc::kill(helper.process.id() as i32, libc::SIGSTOP) }, 0);
    drop(helper);
    let exited_at_drop = cleanup.0.iter().all(exited);
    let reaped_at_drop = cleanup.0.iter().all(reaped);
    sentinel.assert_alive();
    drop(cleanup);
    assert!(
        exited_at_drop,
        "forced helper teardown must terminate escaped family before returning"
    );
    assert!(
        reaped_at_drop,
        "forced helper teardown must reap adopted workload children before returning"
    );
    Ok(())
}

#[test]
fn control_reader_limits_one_frame_when_next_frame_shares_a_read() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let mut command = Process::new("/bin/sh");
    let mut process = command
        .args(["-c", "printf '     \\n{' "])
        .stdout(Stdio::piped())
        .spawn()?;
    let output = process
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing control stdout"))?;
    process.wait()?;
    let mut reader = ControlReader::new(output)?;
    let execution = ExecutionId::new("frame-boundary")?;
    reader.buffered = serde_json::to_vec(&Event::Status {
        execution_id: execution,
        phase: Phase::Running,
    })?;
    reader
        .buffered
        .resize(sandbox_command_supervisor::protocol::MAX_FRAME_BYTES - 6, b' ');
    assert!(matches!(
        reader.next_until(Instant::now() + WAIT)?,
        Event::Status {
            phase: Phase::Running,
            ..
        }
    ));
    assert_eq!(reader.buffered, b"{");
    Ok(())
}

#[test]
fn stalled_control_write_obeys_original_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let mut process = Process::new("/bin/sleep").arg("30").stdin(Stdio::piped()).spawn()?;
    let mut input = process
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing control stdin"))?;
    let writer = std::thread::spawn(move || {
        let started = Instant::now();
        let result = write_control_until(
            &mut input,
            &vec![b'x'; sandbox_command_supervisor::protocol::MAX_FRAME_BYTES + 10],
            started + Duration::from_millis(50),
        );
        (started.elapsed(), result)
    });
    std::thread::sleep(Duration::from_millis(250));
    let _ = process.kill();
    let _ = process.wait();
    let (elapsed, result) = writer.join().map_err(|_| io::Error::other("control writer panicked"))?;
    assert!(result.is_err_and(|error| error.kind() == io::ErrorKind::TimedOut));
    assert!(
        elapsed < Duration::from_millis(200),
        "stalled control write ignored its deadline"
    );
    Ok(())
}

#[test]
fn terminal_exit_assertion_rejects_a_child_held_behind_a_gate() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let custody = FamilyCustody::enter();
    let process = Process::new("/bin/sh")
        .args(["-c", "read -r release"])
        .stdin(Stdio::piped())
        .spawn()?;
    let mut child = BackpressureChild {
        process,
        input: None,
        output: None,
        custody,
    };
    child.input = child.process.stdin.take();
    let pid = child.process.id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let start = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .ok_or_else(|| io::Error::other("child identity"))?
        .parse()?;
    let descriptor = pin(Identity { pid, start_time: start });
    let started = Instant::now();
    let rejected = std::panic::catch_unwind(|| assert_exited(&descriptor));
    let immediate = started.elapsed() < WAIT / 2;
    child.input.take();
    assert_dead(&descriptor);
    assert_exited(&descriptor);
    assert!(rejected.is_err(), "a terminal receipt cannot precede child exit");
    assert!(immediate, "terminal receipt validation waited for a later process exit");
    Ok(())
}
