use super::*;
use std::time::Instant;

pub(crate) struct ControlStall {
    stop: Arc<AtomicBool>,
    stalled: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl PendingListener {
    pub(crate) fn start_control_stall(self, output: &impl AsRawFd, helper: u32) -> io::Result<ControlStall> {
        let descriptor = receive_listener(&self.socket)?;
        let raw = unsafe { libc::fcntl(output.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let output = unsafe { OwnedFd::from_raw_fd(raw) };
        let stop = Arc::new(AtomicBool::new(false));
        let stalled = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_stalled = Arc::clone(&stalled);
        let worker = std::thread::spawn(move || observe(descriptor, output, helper, &worker_stop, &worker_stalled));
        Ok(ControlStall {
            stop,
            stalled,
            worker: Some(worker),
        })
    }
}

impl ControlStall {
    pub(crate) fn wait_until_stalled(&self, until: Instant) -> io::Result<()> {
        while !self.stalled.load(Ordering::Acquire) {
            if self.worker.as_ref().is_none_or(JoinHandle::is_finished) {
                return Err(io::Error::other("control write observer ended before a stalled write"));
            }
            if Instant::now() >= until {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        self.worker
            .take()
            .ok_or_else(|| io::Error::other("missing control write observer"))?
            .join()
            .map_err(|_| io::Error::other("control write observer panicked"))?
    }
}

impl Drop for ControlStall {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn observe(
    descriptor: OwnedFd,
    output: OwnedFd,
    helper: u32,
    stop: &AtomicBool,
    stalled: &AtomicBool,
) -> io::Result<()> {
    let mut last = None;
    let mut policy = Policy::observe();
    while !stop.load(Ordering::Acquire) {
        let mut poll = libc::pollfd {
            fd: descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 20) } < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if poll.revents & libc::POLLHUP != 0 {
            return Ok(());
        }
        if poll.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut request: libc::seccomp_notif = unsafe { zeroed() };
        if unsafe {
            libc::ioctl(
                descriptor.as_raw_fd(),
                libc::_IOWR::<libc::seccomp_notif>(u32::from(b'!'), 0),
                &mut request,
            )
        } < 0
        {
            if matches!(
                io::Error::last_os_error().raw_os_error(),
                Some(libc::EINTR | libc::ENOENT)
            ) {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if request.pid == helper {
            let mut queued = 0_i32;
            if unsafe { libc::ioctl(output.as_raw_fd(), libc::FIONREAD, &mut queued) } < 0 {
                return Err(io::Error::last_os_error());
            }
            let attempt = (request.data.args[1], request.data.args[2], queued);
            if queued > 0 && last == Some(attempt) {
                stalled.store(true, Ordering::Release);
            }
            last = Some(attempt);
        }
        let next = policy;
        if let Err(error) = send_response(&descriptor, &request, &mut policy, next, &Action::Continue)
            && error.raw_os_error() != Some(libc::ENOENT)
        {
            return Err(error);
        }
    }
    Ok(())
}
