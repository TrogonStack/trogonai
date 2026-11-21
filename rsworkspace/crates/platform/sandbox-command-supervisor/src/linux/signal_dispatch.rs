use std::collections::BTreeMap;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

use super::{ErrorCode, errno, parse_children, pidfd, signal_pidfd};

struct Target {
    pid: i32,
    descriptor: OwnedFd,
}

struct Snapshot {
    targets: Vec<Target>,
    seen: BTreeMap<i32, usize>,
    remaining: usize,
    capacity: usize,
}

impl Snapshot {
    fn visit(&mut self) -> Result<(), ErrorCode> {
        self.remaining = self.remaining.checked_sub(1).ok_or(ErrorCode::ProcessLimit)?;
        Ok(())
    }

    fn descend(&mut self, cursor: usize) -> Result<(), ErrorCode> {
        let target = &self.targets[cursor];
        if !alive(&target.descriptor)? {
            return Err(ErrorCode::IdentityUnknown);
        }
        let pid = target.pid;
        let descriptor = target.descriptor.try_clone().map_err(|_| ErrorCode::IdentityUnknown)?;
        let parent = Target { pid, descriptor };
        let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).map_err(|_| ErrorCode::IdentityUnknown)?;
        for task in tasks {
            self.visit()?;
            let task = task.map_err(|_| ErrorCode::IdentityUnknown)?;
            let children = children(&task.path().join("children"), self.capacity)?;
            self.retain(&parent, children)?;
        }
        if !alive(&parent.descriptor)? {
            return Err(ErrorCode::IdentityUnknown);
        }
        Ok(())
    }

    fn retain(&mut self, parent: &Target, children: Vec<i32>) -> Result<(), ErrorCode> {
        for child in children {
            self.visit()?;
            if let Some(index) = self.seen.get(&child) {
                if !alive(&self.targets[*index].descriptor)? {
                    return Err(ErrorCode::IdentityUnknown);
                }
                continue;
            }
            let descriptor = pidfd(child)?;
            let parent_pid = parent_pid(child)?;
            if !alive(&descriptor)? {
                continue;
            }
            // Both procfs paths must still belong to the pinned identities at the relation check.
            if !alive(&parent.descriptor)? || parent_pid != parent.pid {
                return Err(ErrorCode::IdentityUnknown);
            }
            if self.targets.len() == self.capacity {
                return Err(ErrorCode::ProcessLimit);
            }
            self.seen.insert(child, self.targets.len());
            self.targets.push(Target { pid: child, descriptor });
        }
        Ok(())
    }
}

fn alive(descriptor: &OwnedFd) -> Result<bool, ErrorCode> {
    for _ in 0..8 {
        let mut poll = libc::pollfd {
            fd: descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll, 1, 0) };
        if result < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return Err(ErrorCode::IdentityUnknown);
        }
        if poll.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            return Err(ErrorCode::IdentityUnknown);
        }
        return Ok(poll.revents & (libc::POLLIN | libc::POLLHUP) == 0);
    }
    Err(ErrorCode::IdentityUnknown)
}

fn read_bounded(path: &Path, bound: usize) -> Result<Vec<u8>, ErrorCode> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| ErrorCode::IdentityUnknown)?
        .take(bound as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::IdentityUnknown)?;
    if bytes.len() == bound {
        return Err(ErrorCode::ProcessLimit);
    }
    Ok(bytes)
}

fn parent_pid(pid: i32) -> Result<i32, ErrorCode> {
    let bytes = read_bounded(Path::new(&format!("/proc/{pid}/stat")), 4097)?;
    parse_parent(&bytes)
}

fn parse_parent(bytes: &[u8]) -> Result<i32, ErrorCode> {
    let end = bytes
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or(ErrorCode::IdentityUnknown)?;
    let field = bytes[end + 1..]
        .split(u8::is_ascii_whitespace)
        .filter(|field| !field.is_empty())
        .nth(1)
        .ok_or(ErrorCode::IdentityUnknown)?;
    std::str::from_utf8(field)
        .map_err(|_| ErrorCode::IdentityUnknown)?
        .parse::<i32>()
        .map_err(|_| ErrorCode::IdentityUnknown)
}

fn children(path: &Path, limit: usize) -> Result<Vec<i32>, ErrorCode> {
    let bytes = read_bounded(path, limit.saturating_mul(16).saturating_add(1))?;
    let snapshot = parse_children(&bytes, false, limit)?;
    if snapshot.has_more {
        return Err(ErrorCode::ProcessLimit);
    }
    Ok(snapshot.children)
}

fn collect(capacity: usize) -> Result<Vec<Target>, ErrorCode> {
    let supervisor_pid = unsafe { libc::getpid() };
    let supervisor = Target {
        pid: supervisor_pid,
        descriptor: pidfd(supervisor_pid)?,
    };
    let direct = format!("/proc/self/task/{supervisor_pid}/children");
    let mut snapshot = Snapshot {
        targets: Vec::new(),
        seen: BTreeMap::new(),
        // Bound task entries and relation checks independently of the retained process capacity.
        remaining: capacity.saturating_mul(16),
        capacity,
    };
    let mut cursor = 0;
    loop {
        let before = snapshot.targets.len();
        snapshot.retain(&supervisor, children(Path::new(&direct), capacity)?)?;
        while cursor < snapshot.targets.len() {
            snapshot.descend(cursor)?;
            cursor += 1;
        }
        // Adoption can add direct children while other pinned processes are traversed.
        if before == snapshot.targets.len() {
            return Ok(snapshot.targets);
        }
    }
}

pub(super) fn send(capacity: usize, signal: i32) -> Result<bool, ErrorCode> {
    dispatch(collect(capacity)?, signal)
}

fn dispatch(targets: Vec<Target>, signal: i32) -> Result<bool, ErrorCode> {
    let mut delivered = false;
    let mut failure = None;
    for target in targets {
        match signal_pidfd(&target.descriptor, signal) {
            Ok(sent) => delivered |= sent,
            Err(code) => {
                failure.get_or_insert(code);
            }
        }
    }
    failure.map_or(Ok(delivered), Err)
}

#[cfg(any(test, feature = "test-support"))]
pub(in crate::linux) mod tests;
