mod native_child;
mod writer;
use native_child::{Prepared, Root, spawn};
use writer::Writer;

use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use trogon_std::env::{EnumerateEnv, SystemEnv};

use crate::protocol::{
    Command, DEFAULT_RELEASE_TIMEOUT_MS, ErrorCode, Event, ExecutionId, Identity, MAX_CHILDREN, MAX_DATA_BYTES,
    MAX_FRAME_BYTES, Phase, Stream, VERSION, decode_command, encode_event,
};

mod constants;
use constants::{ACK_TIMEOUT, CLEANUP_NOTICE, INIT_TIMEOUT, QUEUED_CONTROL_LIMIT, QUEUED_DATA_LIMIT};
static EXTERNAL_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn remember_signal(signal: i32) {
    EXTERNAL_SIGNAL.store(signal, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
struct RootStatus {
    code: Option<i32>,
    signal: Option<i32>,
}

impl RootStatus {
    fn from_raw(status: i32) -> Option<Self> {
        if libc::WIFEXITED(status) {
            Some(Self {
                code: Some(libc::WEXITSTATUS(status)),
                signal: None,
            })
        } else if libc::WIFSIGNALED(status) {
            Some(Self {
                code: None,
                signal: Some(libc::WTERMSIG(status)),
            })
        } else {
            None
        }
    }
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)
}

fn nonblocking(fd: RawFd) -> Result<(), ErrorCode> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(ErrorCode::IoFailure);
        }
    }
    Ok(())
}

fn pipe() -> Result<(OwnedFd, OwnedFd), ErrorCode> {
    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(ErrorCode::IoFailure);
    }
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    })
}

fn pidfd(pid: i32) -> Result<OwnedFd, ErrorCode> {
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if descriptor < 0 {
        return Err(ErrorCode::IdentityUnknown);
    }
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor as RawFd) })
}

fn signal_pidfd(fd: &OwnedFd, signal: i32) -> Result<bool, ErrorCode> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        return if errno() == libc::ESRCH {
            Ok(false)
        } else {
            Err(ErrorCode::IdentityUnknown)
        };
    }
    Ok(true)
}

fn readable(fd: &OwnedFd) -> Result<bool, ErrorCode> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut poll, 1, 0) };
    if result < 0 {
        if errno() == libc::EINTR {
            return Ok(false);
        }
        return Err(ErrorCode::IoFailure);
    }
    Ok(poll.revents & libc::POLLIN != 0)
}

struct ChildSnapshot {
    children: Vec<i32>,
    has_more: bool,
}

fn parse_children(bytes: &[u8], truncated: bool, limit: usize) -> Result<ChildSnapshot, ErrorCode> {
    let complete_end = bytes
        .iter()
        .rposition(u8::is_ascii_whitespace)
        .map_or(0, |position| position + 1);
    let mut snapshot = ChildSnapshot {
        children: Vec::new(),
        has_more: truncated || complete_end != bytes.len(),
    };
    for text in bytes[..complete_end]
        .split(u8::is_ascii_whitespace)
        .filter(|text| !text.is_empty())
    {
        if snapshot.children.len() == limit {
            snapshot.has_more = true;
            break;
        }
        if !text.iter().all(u8::is_ascii_digit) {
            return Err(ErrorCode::IdentityUnknown);
        }
        let child = std::str::from_utf8(text)
            .map_err(|_| ErrorCode::IdentityUnknown)?
            .parse::<i32>()
            .map_err(|_| ErrorCode::IdentityUnknown)?;
        if child <= 0 {
            return Err(ErrorCode::IdentityUnknown);
        }
        snapshot.children.push(child);
    }
    Ok(snapshot)
}

fn direct_children(limit: usize) -> Result<ChildSnapshot, ErrorCode> {
    let id = unsafe { libc::getpid() };
    let mut value = Vec::new();
    let bound = limit * 16 + 1;
    std::fs::File::open(format!("/proc/self/task/{id}/children"))
        .map_err(|_| ErrorCode::IdentityUnknown)?
        .take(bound as u64)
        .read_to_end(&mut value)
        .map_err(|_| ErrorCode::IdentityUnknown)?;
    parse_children(&value, value.len() == bound, limit)
}

fn identity(pid: i32) -> Result<(Identity, i32, i32), ErrorCode> {
    let value = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|_| ErrorCode::IdentityUnknown)?;
    let end = value.rfind(')').ok_or(ErrorCode::IdentityUnknown)?;
    let fields: Vec<_> = value[end + 1..].split_whitespace().collect();
    let field = |index: usize| fields.get(index).ok_or(ErrorCode::IdentityUnknown);
    let group = field(2)?.parse::<i32>().map_err(|_| ErrorCode::IdentityUnknown)?;
    let session = field(3)?.parse::<i32>().map_err(|_| ErrorCode::IdentityUnknown)?;
    let start_time = field(19)?.parse::<u64>().map_err(|_| ErrorCode::IdentityUnknown)?;
    Ok((
        Identity {
            pid: pid as u32,
            start_time,
        },
        group,
        session,
    ))
}

unsafe fn disposition(signal: i32, handler: libc::sighandler_t) -> Result<(), ErrorCode> {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
    }
    action.sa_flags = 0;
    if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } < 0 {
        return Err(ErrorCode::KernelPrerequisite);
    }
    Ok(())
}

pub fn probe() -> Result<(), ErrorCode> {
    unsafe {
        if libc::getpid() <= 1
            || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) < 0
            || libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) < 0
            || libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0
        {
            return Err(ErrorCode::KernelPrerequisite);
        }
        disposition(libc::SIGCHLD, libc::SIG_DFL)?;
        disposition(libc::SIGPIPE, libc::SIG_IGN)?;
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT] {
            disposition(signal, remember_signal as *const () as libc::sighandler_t)?;
        }
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        if libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) < 0 {
            return Err(ErrorCode::KernelPrerequisite);
        }
        let mut stat: libc::statfs = std::mem::zeroed();
        if libc::statfs(c"/proc".as_ptr(), &mut stat) < 0 || stat.f_type as u64 != 0x9fa0 {
            return Err(ErrorCode::KernelPrerequisite);
        }
        let children = direct_children(MAX_CHILDREN)?;
        if !children.children.is_empty() || children.has_more {
            return Err(ErrorCode::KernelPrerequisite);
        }
        let own = pidfd(libc::getpid())?;
        signal_pidfd(&own, 0)?;
        // Child descriptor isolation uses the same syscall before any user exec.
        if libc::syscall(libc::SYS_close_range, u32::MAX, u32::MAX, 0) < 0 {
            return Err(ErrorCode::KernelPrerequisite);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SignalRequest {
    request_id: u64,
    signal: i32,
    outcome: Option<Result<bool, ErrorCode>>,
}

struct Supervisor {
    family_limit: usize,
    execution: ExecutionId,
    phase: Phase,
    root: Option<Root>,
    writer: Writer,
    input: Vec<u8>,
    discard_input: bool,
    input_closed: bool,
    released: bool,
    canceled: bool,
    timed_out: bool,
    reaped: u64,
    entered: Instant,
    release_deadline: Option<Instant>,
    command_timeout: Option<Duration>,
    command_deadline: Option<Instant>,
    cleanup_started: Option<Instant>,
    cleanup_notice: bool,
    process_limit_notice: bool,
    terminal_at: Option<Instant>,
    acknowledged: bool,
    signal_request: Option<SignalRequest>,
}

impl Supervisor {
    fn new(execution: ExecutionId, family_limit: usize) -> Self {
        Self {
            execution,
            family_limit,
            phase: Phase::AwaitingInit,
            root: None,
            writer: Writer::new(),
            input: Vec::new(),
            discard_input: false,
            input_closed: false,
            released: false,
            canceled: false,
            timed_out: false,
            reaped: 0,
            entered: Instant::now(),
            release_deadline: None,
            command_timeout: None,
            command_deadline: None,
            cleanup_started: None,
            cleanup_notice: false,
            process_limit_notice: false,
            terminal_at: None,
            acknowledged: false,
            signal_request: None,
        }
    }

    fn error(&mut self, code: ErrorCode, uncertain: bool) {
        if self
            .writer
            .queue(Event::Error {
                execution_id: self.execution.clone(),
                code,
                uncertain: uncertain.into(),
            })
            .is_err()
        {
            self.writer.closed = true;
            self.begin_drain();
        }
    }

    fn begin_drain(&mut self) {
        if matches!(self.phase, Phase::Absent) {
            return;
        }
        self.canceled = true;
        self.phase = Phase::Draining;
        self.cleanup_started.get_or_insert_with(Instant::now);
        if let Some(root) = &mut self.root {
            root.gate.take();
        }
    }

    fn process_limit(&mut self) {
        if !self.process_limit_notice {
            self.process_limit_notice = true;
            self.error(ErrorCode::ProcessLimit, true);
        }
        self.begin_drain();
    }

    fn command(&mut self, command: Command) {
        if command.execution_id() != &self.execution {
            self.error(ErrorCode::WrongExecution, false);
            return;
        }
        match command {
            Command::Init { .. } if self.phase == Phase::AwaitingInit => {
                let prepared = match Prepared::from_command(command) {
                    Ok(prepared) => prepared,
                    Err(code) => {
                        self.error(code, false);
                        return;
                    }
                };
                match spawn(&prepared) {
                    Ok(root) => {
                        let helper = match identity(unsafe { libc::getpid() }) {
                            Ok((identity, _, _)) => identity,
                            Err(code) => {
                                self.root = Some(root);
                                self.error(code, true);
                                self.begin_drain();
                                return;
                            }
                        };
                        let event = Event::Ready {
                            version: VERSION,
                            execution_id: self.execution.clone(),
                            helper,
                            root: root.identity,
                            process_group: root.pid,
                            session: root.pid,
                        };
                        self.root = Some(root);
                        self.phase = Phase::Held;
                        self.command_timeout = prepared.timeout;
                        self.release_deadline = Some(Instant::now() + prepared.release_timeout);
                        if self.writer.queue(event).is_err() {
                            self.begin_drain();
                        }
                    }
                    Err(code) => {
                        self.error(code, true);
                        self.begin_drain();
                    }
                }
            }
            Command::Release { .. } if self.phase == Phase::Held => {
                let result = self
                    .root
                    .as_mut()
                    .and_then(|root| root.gate.take())
                    .map(|gate| unsafe { libc::write(gate.as_raw_fd(), b"R".as_ptr().cast(), 1) == 1 })
                    .unwrap_or(false);
                if !result {
                    self.error(ErrorCode::IoFailure, true);
                    self.begin_drain();
                    return;
                }
                self.released = true;
                self.phase = Phase::Running;
                self.command_deadline = self.command_timeout.map(|timeout| Instant::now() + timeout);
                let _ = self.writer.queue(Event::Released {
                    execution_id: self.execution.clone(),
                });
            }
            Command::Signal { request_id, signal, .. } if self.phase != Phase::AwaitingInit => {
                let request_id = request_id.get();
                let signal = signal.get();
                if let Some(previous) = self.signal_request
                    && request_id <= previous.request_id
                {
                    if request_id != previous.request_id || signal != previous.signal {
                        self.error(ErrorCode::InvalidCommand, false);
                    } else {
                        self.signal_result(
                            request_id,
                            signal,
                            previous.outcome.unwrap_or(Err(ErrorCode::CleanupPending)),
                        );
                    }
                    return;
                }
                self.signal_request = Some(SignalRequest {
                    request_id,
                    signal,
                    outcome: None,
                });
                let result = if self.phase == Phase::Absent {
                    Ok(false)
                } else if signal == libc::SIGKILL {
                    self.begin_drain();
                    self.hard_signal()
                } else {
                    self.soft_signal(signal)
                };
                self.signal_request = Some(SignalRequest {
                    request_id,
                    signal,
                    outcome: Some(result),
                });
                self.signal_result(request_id, signal, result);
            }
            Command::Status { .. } => self.status(),
            Command::Acknowledge { .. } if self.phase == Phase::Absent => {
                self.acknowledged = true;
            }
            _ => self.error(ErrorCode::WrongPhase, false),
        }
    }

    fn signal_result(&mut self, request_id: u64, signal: i32, result: Result<bool, ErrorCode>) {
        match result {
            Ok(delivered) => {
                if self
                    .writer
                    .queue(Event::SignalResult {
                        execution_id: self.execution.clone(),
                        request_id,
                        signal,
                        delivered: delivered.into(),
                    })
                    .is_err()
                {
                    self.begin_drain();
                }
                if self.phase == Phase::Absent {
                    self.status();
                }
            }
            Err(code) => {
                self.error(code, true);
                self.begin_drain();
            }
        }
    }

    fn status(&mut self) {
        let _ = self.writer.queue(Event::Status {
            execution_id: self.execution.clone(),
            phase: self.phase,
        });
    }

    fn soft_signal(&mut self, signal: i32) -> Result<bool, ErrorCode> {
        let mut delivered = false;
        if let Some(root) = &self.root {
            if !root.reaped {
                // The sole reaper keeps this direct child waitable, reserving its private session.
                let result = unsafe { libc::kill(-root.pid, signal) };
                if result < 0 && errno() != libc::ESRCH {
                    return Err(ErrorCode::IdentityUnknown);
                }
                delivered |= result == 0;
            }
            let children = direct_children(self.family_limit)?;
            for child in children.children {
                if child != root.pid {
                    let descriptor = pidfd(child)?;
                    delivered |= signal_pidfd(&descriptor, signal)?;
                }
            }
            if children.has_more {
                return Err(ErrorCode::ProcessLimit);
            }
        }
        Ok(delivered)
    }

    fn hard_signal(&mut self) -> Result<bool, ErrorCode> {
        let mut delivered = false;
        let root_pid = self.root.as_ref().map(|root| root.pid);
        if let Some(root) = &self.root
            && !root.reaped
        {
            delivered |= signal_pidfd(&root.pidfd, libc::SIGKILL)?;
        }
        let children = direct_children(self.family_limit)?;
        if children.has_more {
            self.process_limit();
        }
        for child in children.children {
            if Some(child) != root_pid {
                let descriptor = pidfd(child)?;
                delivered |= signal_pidfd(&descriptor, libc::SIGKILL)?;
            }
        }
        Ok(delivered)
    }

    fn read_input(&mut self) {
        let mut bytes = [0_u8; 8192];
        for _ in 0..8 {
            let count = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                self.input_closed = true;
                self.begin_drain();
                return;
            }
            if count < 0 {
                match errno() {
                    libc::EAGAIN | libc::EINTR => return,
                    _ => {
                        self.input_closed = true;
                        self.begin_drain();
                        return;
                    }
                }
            }
            for byte in &bytes[..count as usize] {
                if self.discard_input {
                    if *byte == b'\n' {
                        self.discard_input = false;
                    }
                    continue;
                }
                if *byte == b'\n' {
                    let input = std::mem::take(&mut self.input);
                    match decode_command(&input) {
                        Ok(command) => self.command(command),
                        Err(_) => self.error(ErrorCode::InvalidFrame, false),
                    }
                } else if self.input.len() < MAX_FRAME_BYTES {
                    self.input.push(*byte);
                } else {
                    self.input.clear();
                    self.discard_input = true;
                    self.error(ErrorCode::InvalidFrame, false);
                }
            }
        }
    }

    fn read_output(&mut self, stream: Stream) {
        for _ in 0..8 {
            let descriptor = self.root.as_ref().and_then(|root| match stream {
                Stream::Stdout => root.stdout.as_ref(),
                Stream::Stderr => root.stderr.as_ref(),
            });
            let Some(descriptor) = descriptor else {
                return;
            };
            let mut bytes = [0_u8; MAX_DATA_BYTES];
            let count = unsafe { libc::read(descriptor.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                if let Some(root) = &mut self.root {
                    match stream {
                        Stream::Stdout => {
                            root.stdout.take();
                        }
                        Stream::Stderr => {
                            root.stderr.take();
                        }
                    }
                }
                return;
            }
            if count < 0 {
                match errno() {
                    libc::EAGAIN | libc::EINTR => return,
                    _ => {
                        self.writer.truncate(stream);
                        return;
                    }
                }
            }
            let _ = self.writer.queue(Event::Output {
                execution_id: self.execution.clone(),
                stream,
                data: bytes[..count as usize].to_vec(),
            });
        }
    }

    fn observe_exec(&mut self) {
        if self.phase == Phase::Absent {
            return;
        }
        let Some(root) = &self.root else {
            return;
        };
        let mut data = [0_u8; 5];
        let count = unsafe { libc::read(root.setup.as_raw_fd(), data.as_mut_ptr().cast(), data.len()) };
        if count > 0 {
            self.error(ErrorCode::ExecFailed, false);
        }
    }

    fn tick_family(&mut self) -> Result<(), ErrorCode> {
        if self.phase == Phase::Draining {
            self.hard_signal()?;
        }
        let children = direct_children(self.family_limit)?;
        let root_pid = self.root.as_ref().map(|root| root.pid);
        if children.has_more {
            self.process_limit();
            self.hard_signal()?;
        }
        for child in &children.children {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    *child as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT | libc::__WALL,
                )
            };
            if result < 0 {
                return Err(ErrorCode::IdentityUnknown);
            }
            if unsafe { info.si_pid() } == 0 {
                continue;
            }
            if Some(*child) == root_pid {
                let Some(root) = self.root.as_mut() else {
                    return Err(ErrorCode::IdentityUnknown);
                };
                if root.status.is_none() && readable(&root.pidfd)? {
                    let code = info.si_code;
                    let value = unsafe { info.si_status() };
                    let status = if code == libc::CLD_EXITED {
                        RootStatus {
                            code: Some(value),
                            signal: None,
                        }
                    } else if code == libc::CLD_KILLED || code == libc::CLD_DUMPED {
                        RootStatus {
                            code: None,
                            signal: Some(value),
                        }
                    } else {
                        return Err(ErrorCode::IdentityUnknown);
                    };
                    root.status = Some(status);
                    if self.released {
                        let _ = self.writer.queue(Event::RootExited {
                            execution_id: self.execution.clone(),
                            exit_code: status.code,
                            signal: status.signal,
                        });
                    }
                }
            } else {
                let descriptor = pidfd(*child)?;
                if !readable(&descriptor)? {
                    continue;
                }
                let mut status = 0;
                let result = unsafe { libc::waitpid(*child, &mut status, libc::__WALL | libc::WNOHANG) };
                if result == *child {
                    self.reaped = self.reaped.saturating_add(1);
                } else if result < 0 {
                    return Err(ErrorCode::IdentityUnknown);
                }
            }
        }
        let children = direct_children(self.family_limit)?;
        if let Some(root) = &mut self.root
            && !root.reaped
            && root.status.is_some()
            && !children.has_more
            && children.children.iter().all(|child| *child == root.pid)
        {
            let mut status = 0;
            let result = unsafe { libc::waitpid(root.pid, &mut status, libc::__WALL | libc::WNOHANG) };
            if result != root.pid || RootStatus::from_raw(status).is_none() {
                return Err(ErrorCode::IdentityUnknown);
            }
            root.reaped = true;
            self.reaped = self.reaped.saturating_add(1);
        }
        // Never reap an identity appearing after the snapshot without first pinning it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_ALL,
                0,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT | libc::__WALL,
            )
        };
        if result == 0 {
            return Ok(());
        }
        if errno() != libc::ECHILD {
            return Err(ErrorCode::IdentityUnknown);
        }
        if let Some(root) = &self.root
            && (!root.reaped || !readable(&root.pidfd)?)
        {
            return Err(ErrorCode::IdentityUnknown);
        }
        if self.phase == Phase::AwaitingInit {
            return Ok(());
        }
        self.observe_exec();
        self.read_output(Stream::Stdout);
        self.read_output(Stream::Stderr);
        if let Some(root) = &mut self.root {
            if root.stdout.take().is_some() {
                self.writer.truncate(Stream::Stdout);
            }
            if root.stderr.take().is_some() {
                self.writer.truncate(Stream::Stderr);
            }
        }
        let status = self.root.as_ref().and_then(|root| root.status);
        self.phase = Phase::Absent;
        self.terminal_at = Some(Instant::now());
        self.writer.queue(Event::FamilyAbsent {
            execution_id: self.execution.clone(),
            released: self.released.into(),
            root_exit_code: self.released.then_some(status).flatten().and_then(|status| status.code),
            root_signal: self
                .released
                .then_some(status)
                .flatten()
                .and_then(|status| status.signal),
            timed_out: self.timed_out.into(),
            canceled: self.canceled.into(),
            reaped_count: self.reaped,
            stdout_truncated: self.writer.stdout_truncated.into(),
            stderr_truncated: self.writer.stderr_truncated.into(),
        })?;
        Ok(())
    }

    fn run_loop(&mut self) -> i32 {
        loop {
            self.writer.flush();
            if self.writer.closed && !self.input_closed {
                self.input_closed = true;
                self.begin_drain();
            }
            if !self.input_closed {
                self.read_input();
            }
            let signal = EXTERNAL_SIGNAL.swap(0, Ordering::Relaxed);
            if signal != 0
                && let Err(code) = self.soft_signal(signal)
            {
                self.error(code, true);
            }
            if self.phase == Phase::AwaitingInit && (self.input_closed || self.entered.elapsed() >= INIT_TIMEOUT) {
                self.writer.flush();
                return if self.input_closed { 0 } else { 64 };
            }
            if self.phase == Phase::Held && self.release_deadline.is_some_and(|time| Instant::now() >= time) {
                self.timed_out = true;
                self.begin_drain();
            }
            if self.phase == Phase::Running && self.command_deadline.is_some_and(|time| Instant::now() >= time) {
                self.timed_out = true;
                self.begin_drain();
            }
            self.read_output(Stream::Stdout);
            self.read_output(Stream::Stderr);
            self.observe_exec();
            if !matches!(self.phase, Phase::AwaitingInit | Phase::Absent)
                && let Err(code) = self.tick_family()
            {
                self.error(code, true);
                self.begin_drain();
            }
            if self.phase == Phase::Draining
                && !self.cleanup_notice
                && self
                    .cleanup_started
                    .is_some_and(|time| time.elapsed() >= CLEANUP_NOTICE)
            {
                self.cleanup_notice = true;
                self.error(ErrorCode::CleanupPending, true);
            }
            self.writer.flush();
            if self.phase == Phase::Absent
                && (self.acknowledged
                    || self.input_closed
                    || self.terminal_at.is_some_and(|time| time.elapsed() >= ACK_TIMEOUT))
            {
                if self.writer.pending.is_empty() || self.writer.closed {
                    return 0;
                }
                if self.terminal_at.is_some_and(|time| time.elapsed() >= ACK_TIMEOUT) {
                    return 74;
                }
            }
            let mut polls = [
                libc::pollfd {
                    fd: if self.input_closed { -1 } else { 0 },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if self.writer.pending.is_empty() || self.writer.closed {
                        -1
                    } else {
                        1
                    },
                    events: libc::POLLOUT,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self
                        .root
                        .as_ref()
                        .and_then(|root| root.stdout.as_ref())
                        .map_or(-1, AsRawFd::as_raw_fd),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self
                        .root
                        .as_ref()
                        .and_then(|root| root.stderr.as_ref())
                        .map_or(-1, AsRawFd::as_raw_fd),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            unsafe {
                libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 20);
            }
        }
    }
}

pub fn run(execution: ExecutionId, family_limit: usize) -> i32 {
    let mut supervisor = Supervisor::new(execution.clone(), family_limit);
    if let Err(code) = probe().and_then(|_| nonblocking(0)).and_then(|_| nonblocking(1)) {
        let event = Event::Error {
            execution_id: execution,
            code,
            uncertain: false.into(),
        };
        if let Ok(frame) = encode_event(&event) {
            unsafe {
                libc::write(1, frame.as_ptr().cast(), frame.len());
            }
        }
        return 78;
    }
    let environment: Result<BTreeMap<_, _>, ErrorCode> = SystemEnv
        .vars_os()
        .into_iter()
        .map(|(key, value)| {
            Ok((
                key.into_string().map_err(|_| ErrorCode::InvalidCommand)?,
                value.into_string().map_err(|_| ErrorCode::InvalidCommand)?,
            ))
        })
        .collect();
    let environment = match environment {
        Ok(environment) if crate::Environment::new(environment.clone()).is_ok() => environment,
        _ => {
            supervisor.error(ErrorCode::InvalidCommand, false);
            supervisor.writer.flush();
            return 64;
        }
    };
    if supervisor
        .writer
        .queue(Event::Hello {
            version: VERSION,
            execution_id: execution,
            environment,
        })
        .is_err()
    {
        return 74;
    }
    supervisor.run_loop()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod state_tests;
