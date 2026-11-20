use super::*;
use trogon_std::env::ReadEnv;

#[test]
fn missing_kernel_notification_preserves_allow_and_one_shot_refusal() -> Result<(), Box<dyn std::error::Error>> {
    const CASE: &str = "syscall_notifications::tests::missing_kernel_notification_preserves_allow_and_one_shot_refusal";
    if trogon_std::env::SystemEnv
        .var_os("SUPERVISOR_NOTIFICATION_CASE")
        .is_some()
    {
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
    command
        .args(["--exact", CASE, "--nocapture"])
        .env("SUPERVISOR_NOTIFICATION_CASE", "1");
    let pending = PendingListener::install(&mut command, Syscall::Session, Policy::observe())?;
    let mut target = Target(command.spawn()?);
    let descriptor = receive_listener(&pending.socket)?;
    let mut policy = Policy::reject_once_after(1, libc::EAGAIN);
    for expected_refusal in [false, true] {
        let until = std::time::Instant::now() + super::super::WAIT;
        let mut request: libc::seccomp_notif = unsafe { zeroed() };
        loop {
            let mut poll = libc::pollfd {
                fd: descriptor.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            let result = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().min(i32::MAX as u128) as i32) };
            if result < 0 {
                return Err(io::Error::last_os_error().into());
            }
            if result == 0 {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "kernel notification did not arrive").into());
            }
            if unsafe {
                libc::ioctl(
                    descriptor.as_raw_fd(),
                    libc::_IOWR::<libc::seccomp_notif>(u32::from(b'!'), 0),
                    &mut request,
                )
            } == 0
            {
                break;
            }
            if !matches!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::EINTR | libc::ENOENT)
            ) {
                return Err(io::Error::last_os_error().into());
            }
        }
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
