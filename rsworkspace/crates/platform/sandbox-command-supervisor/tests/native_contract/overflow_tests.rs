use super::*;
use sandbox_command_supervisor::protocol::MAX_CHILDREN;

type Subreaper = TestCustody;

struct Family {
    helper: Harness,
    identities: Vec<OwnedFd>,
    cleaned: bool,
    _subreaper: Subreaper,
}

impl Family {
    fn start() -> Self {
        let subreaper = Subreaper::enter();
        Self {
            helper: Harness::start_binary(env!("CARGO_BIN_EXE_supervisor-overflow-fixture")),
            identities: Vec::new(),
            cleaned: false,
            _subreaper: subreaper,
        }
    }

    fn cleanup(&mut self) -> bool {
        if self.cleaned {
            return true;
        }
        for identity in &self.identities {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    identity.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
        self.helper.input.take();
        let helper_absent = stop_helper(&mut self.helper.process, Duration::ZERO);
        let until = Instant::now() + Duration::from_secs(3);
        let settled = self.helper.custody.settle();
        let mut complete = helper_absent && settled;
        for identity in &self.identities {
            let mut reaped = false;
            while Instant::now() < until {
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let result = unsafe {
                    libc::waitid(
                        libc::P_PIDFD,
                        identity.as_raw_fd() as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::__WALL,
                    )
                };
                if result == 0 && unsafe { info.si_pid() } > 0 {
                    reaped = true;
                    break;
                }
                if result < 0 {
                    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                        reaped = dead(identity, Instant::now());
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            complete &= reaped && dead(identity, Instant::now());
        }
        self.cleaned = complete;
        if complete {
            eprintln!("overflow fixture: exact pinned family teardown complete");
        } else {
            eprintln!("overflow fixture: exact pinned family teardown incomplete");
        }
        complete
    }
}

impl Drop for Family {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn dead(identity: &OwnedFd, until: Instant) -> bool {
    let remaining = until.saturating_duration_since(Instant::now());
    let mut poll = libc::pollfd {
        fd: identity.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().min(i32::MAX as u128) as i32) };
    result == 1 && poll.revents & libc::POLLIN != 0
}

#[derive(Clone, Copy)]
enum Control {
    Eof,
    Deadline,
    HardCancel,
}

#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn run(control: Control) {
    assert_eq!(MAX_CHILDREN, 4096, "test support must preserve production capacity");
    let directory = tempfile::tempdir().unwrap();
    let mut sentinel = Sentinel::start();
    let mut family = Family::start();
    let timeout = matches!(control, Control::Deadline).then_some(5_000);
    let root = family.helper.initialize("overflow", directory.path(), timeout, None);
    family.identities.push(pin(root));
    family.helper.release();
    let actual_root = recorded(&directory.path().join("root"));
    assert_eq!(actual_root.pid, root.pid);
    assert_eq!(actual_root.start_time, root.start_time);
    family.identities.push(pin(recorded(&directory.path().join("middle"))));
    for index in 0..10 {
        family
            .identities
            .push(pin(recorded(&directory.path().join(format!("leaf-{index}")))));
    }
    assert!(family.identities.iter().all(|identity| !dead(identity, Instant::now())));
    sentinel.assert_alive();
    std::fs::write(directory.path().join("adopt"), b"release").unwrap();
    assert!(
        matches!(
            family.helper.next(),
            Event::Error {
                code: ErrorCode::ProcessLimit,
                uncertain: Certainty::Uncertain,
                ..
            }
        ),
        "real kernel family must cross the bound"
    );
    match control {
        Control::Eof => {
            family.helper.input.take();
        }
        Control::Deadline => {}
        Control::HardCancel => family.helper.send(Command::Signal {
            execution_id: family.helper.execution.clone(),
            request_id: 1,
            signal: libc::SIGKILL,
        }),
    }
    let until = Instant::now() + WAIT;
    let root_absent = dead(&family.identities[0], until);
    let mut descendants_absent = true;
    for identity in &family.identities[1..] {
        descendants_absent &= dead(identity, until);
    }
    let mut terminal = false;
    if root_absent && descendants_absent {
        let until = Instant::now() + WAIT;
        while Instant::now() < until {
            match family
                .helper
                .events
                .recv_timeout(until.saturating_duration_since(Instant::now()))
            {
                Ok(Event::FamilyAbsent {
                    released: ReleaseState::Released,
                    canceled: Cancellation::Canceled,
                    root_signal: Some(libc::SIGKILL),
                    reaped_count,
                    ..
                }) if reaped_count >= 11 => {
                    terminal = true;
                    break;
                }
                Ok(Event::Error {
                    code: ErrorCode::ProcessLimit,
                    ..
                })
                | Ok(Event::RootExited { .. })
                | Ok(Event::SignalResult { .. })
                | Ok(Event::Status { .. })
                | Ok(Event::Output { .. }) => {}
                _ => break,
            }
        }
    }
    if terminal {
        if family.helper.input.is_some() {
            family.helper.send(Command::Acknowledge {
                execution_id: family.helper.execution.clone(),
            });
        }
        family.helper.wait();
    }
    sentinel.assert_alive();
    let fixture_clean = family.cleanup();
    assert!(
        fixture_clean,
        "exact overflow fixture cleanup must finish even on semantic failure"
    );
    sentinel.assert_alive();
    assert!(root_absent, "over-limit control left the pinned original root alive");
    assert!(
        descendants_absent,
        "over-limit control left pinned adopted descendants alive"
    );
    assert!(terminal, "over-limit family requires genuine terminal absence");
}

#[test]
fn overflow_control_eof_drains_pinned_family() {
    let _custody = TestCustody::enter();
    run(Control::Eof);
}

#[test]
fn overflow_finite_command_deadline_drains_pinned_family() {
    let _custody = TestCustody::enter();
    run(Control::Deadline);
}

#[test]
fn overflow_explicit_hard_cancel_drains_pinned_family() {
    let _custody = TestCustody::enter();
    run(Control::HardCancel);
}

#[test]
fn subreaper_custody_excludes_a_second_owner_until_restoration() -> Result<(), Box<dyn std::error::Error>> {
    let first = Subreaper::enter();
    let (send, receive) = std::sync::mpsc::channel();
    let second = std::thread::spawn(move || {
        let _ = send.send(TestCustody::unavailable_to_another_thread());
        let guard = Subreaper::enter();
        let _ = send.send(false);
        drop(guard);
    });
    let blocked = receive.recv_timeout(WAIT)?;
    drop(first);
    assert!(!receive.recv_timeout(WAIT)?);
    second
        .join()
        .map_err(|_| std::io::Error::other("subreaper owner panicked"))?;
    assert!(blocked, "a second owner must observe the held custody lock");
    Ok(())
}

#[test]
fn failed_overflow_setup_reaps_leaves_not_yet_registered_with_the_family() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let directory = tempfile::tempdir()?;
    let mut sentinel = Sentinel::start();
    let mut family = Family::start();
    let root = family.helper.initialize("overflow", directory.path(), None, None);
    family.identities.push(pin(root));
    family.helper.release();
    let mut unregistered = Vec::new();
    for index in 0..10 {
        unregistered.push(pin(recorded(&directory.path().join(format!("leaf-{index}")))));
    }
    assert!(unregistered.iter().all(|identity| !dead(identity, Instant::now())));
    assert!(family.cleanup());
    let cleaned = unregistered.iter().all(|identity| {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PIDFD,
                identity.as_raw_fd() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT | libc::__WALL,
            )
        };
        dead(identity, Instant::now())
            && result < 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
    });
    let independent_cleanup = family.helper.custody.settle();
    sentinel.assert_alive();
    assert!(
        independent_cleanup,
        "failed setup fixture guard must retain all child custody"
    );
    assert!(
        cleaned,
        "overflow cleanup missed leaves started before identity registration"
    );
    Ok(())
}
