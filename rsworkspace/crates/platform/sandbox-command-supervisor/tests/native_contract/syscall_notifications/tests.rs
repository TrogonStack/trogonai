use super::super::TestCustody;
use super::*;
use trogon_std::env::ReadEnv;

#[test]
fn missing_kernel_notification_preserves_allow_and_one_shot_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    const CASE: &str = "syscall_notifications::tests::missing_kernel_notification_preserves_allow_and_one_shot_refusal";
    if inherited_notification_handoff() {
        assert!(unsafe { libc::setsid() } >= 0);
        assert_eq!(unsafe { libc::setsid() }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EAGAIN));
        return Ok(());
    }
    struct Target(std::process::Child);
    impl Drop for Target {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", CASE, "--nocapture"]);
    prepare_notification_handoff(&mut command)?;
    let pending = PendingListener::install(&mut command, Syscall::Session, Policy::observe())?;
    let mut target = Target(command.spawn()?);
    let descriptor = receive_listener(&pending.socket)?;
    let mut policy = Policy::reject_once_after(1, libc::EAGAIN);
    for expected_refusal in [false, true] {
        let until = std::time::Instant::now() + super::super::WAIT;
        let request = receive_notification_until(&descriptor, until)?;
        let mut missing = request;
        missing.id = request.id.wrapping_add(1);
        let mut staged = policy;
        let action = staged.next();
        assert!(matches!(
            (&action, expected_refusal),
            (Action::Continue, false) | (Action::Refuse(libc::EAGAIN), true)
        ));
        assert_eq!(
            send_response(&descriptor, &missing, &mut policy, staged, &action)
                .err()
                .and_then(|error| error.raw_os_error()),
            Some(libc::ENOENT)
        );
        let mut staged = policy;
        let action = staged.next();
        assert!(
            matches!(
                (&action, expected_refusal),
                (Action::Continue, false) | (Action::Refuse(libc::EAGAIN), true)
            ),
            "withdrawn notification must not consume policy"
        );
        send_response(&descriptor, &request, &mut policy, staged, &action)?;
    }
    let until = std::time::Instant::now() + super::super::WAIT;
    loop {
        if let Some(status) = target.0.try_wait()? {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= until {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "notification target did not exit").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}

fn receive_notification_until(descriptor: &OwnedFd, until: std::time::Instant) -> io::Result<libc::seccomp_notif> {
    loop {
        let remaining = until
            .checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
        let mut poll = libc::pollfd {
            fd: descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().clamp(1, i32::MAX as u128) as i32) };
        if result < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if poll.revents & libc::POLLHUP != 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        if poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("kernel notification listener failed"));
        }
        if poll.revents & libc::POLLIN == 0 {
            continue;
        }
        if std::time::Instant::now() >= until {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        let mut request: libc::seccomp_notif = unsafe { zeroed() };
        if unsafe {
            libc::ioctl(
                descriptor.as_raw_fd(),
                libc::_IOWR::<libc::seccomp_notif>(u32::from(b'!'), 0),
                &mut request,
            )
        } == 0
        {
            return Ok(request);
        }
        if !matches!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::EINTR | libc::ENOENT)
        ) {
            return Err(io::Error::last_os_error());
        }
    }
}

const HANDOFF: &[u8] = b"supervisor-notification-worker";

fn prepare_notification_handoff(command: &mut Command) -> io::Result<()> {
    let (sender, receiver) = UnixDatagram::pair()?;
    sender.send(HANDOFF)?;
    command
        .env("SUPERVISOR_NOTIFICATION_CASE", "1")
        .env("SUPERVISOR_NOTIFICATION_HANDOFF", receiver.as_raw_fd().to_string());
    unsafe {
        command.pre_exec(move || {
            let descriptor = receiver.as_raw_fd();
            let flags = libc::fcntl(descriptor, libc::F_GETFD);
            if flags < 0 || libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

fn inherited_notification_handoff() -> bool {
    let environment = trogon_std::env::SystemEnv;
    if environment.var_os("SUPERVISOR_NOTIFICATION_CASE").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return false;
    }
    let Some(raw) = environment
        .var_os("SUPERVISOR_NOTIFICATION_HANDOFF")
        .and_then(|value| value.to_str().and_then(|value| value.parse::<RawFd>().ok()))
    else {
        return false;
    };
    let mut kind = 0_i32;
    let mut length = size_of::<i32>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            raw,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut i32).cast(),
            &mut length,
        )
    } < 0
        || kind != libc::SOCK_DGRAM
    {
        return false;
    }
    let mut bytes = [0_u8; 64];
    let received = unsafe { libc::recv(raw, bytes.as_mut_ptr().cast(), bytes.len(), libc::MSG_DONTWAIT) };
    if received != HANDOFF.len() as isize || &bytes[..HANDOFF.len()] != HANDOFF {
        return false;
    }
    drop(unsafe { OwnedFd::from_raw_fd(raw) });
    true
}

#[test]
fn notification_hangup_stops_before_receive_ioctl() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let (socket, peer) = std::os::unix::net::UnixStream::pair()?;
    drop(peer);
    let descriptor: OwnedFd = socket.into();
    let result = receive_notification_until(&descriptor, std::time::Instant::now() + super::super::WAIT);
    assert!(
        result.is_err_and(|error| error.kind() == io::ErrorKind::UnexpectedEof),
        "listener hangup must not call the receive ioctl"
    );
    Ok(())
}

#[test]
fn inherited_case_environment_alone_cannot_enter_notification_worker() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "syscall_notifications::tests::missing_kernel_notification_preserves_allow_and_one_shot_refusal",
            "--nocapture",
        ])
        .env("SUPERVISOR_NOTIFICATION_CASE", "1")
        .env_remove("SUPERVISOR_NOTIFICATION_HANDOFF");
    let output = super::super::fixture_tests::capture(&mut command)?;
    assert!(
        output.status.success(),
        "inherited case variable entered the child-only setsid path: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(())
}

#[test]
fn withdrawn_notification_before_pidfd_open_keeps_policy_unconsumed() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let mut command = Command::new("/usr/bin/setsid");
    command.arg("/bin/true");
    let pending = PendingListener::install(&mut command, Syscall::Session, Policy::observe())?;
    let mut target = command.spawn()?;
    let descriptor = receive_listener(&pending.socket)?;
    let request = receive_notification_until(&descriptor, std::time::Instant::now() + super::super::WAIT)?;
    target.kill()?;
    target.wait()?;
    let result = pin_notification_target(&descriptor, &request)?;
    assert!(result.is_none(), "a withdrawn request must not end observation");
    let mut policy = Policy::reject_once_after(0, libc::EAGAIN);
    assert!(matches!(policy.next(), Action::Refuse(libc::EAGAIN)));
    Ok(())
}

#[test]
fn notification_ready_descriptor_cannot_extend_an_expired_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let (socket, mut peer) = std::os::unix::net::UnixStream::pair()?;
    std::io::Write::write_all(&mut peer, b"ready")?;
    let descriptor: OwnedFd = socket.into();
    let result = receive_notification_until(
        &descriptor,
        std::time::Instant::now() - std::time::Duration::from_millis(1),
    );
    assert!(
        result.is_err_and(|error| error.kind() == io::ErrorKind::TimedOut),
        "ready events must not erase an expired notification deadline"
    );
    Ok(())
}
