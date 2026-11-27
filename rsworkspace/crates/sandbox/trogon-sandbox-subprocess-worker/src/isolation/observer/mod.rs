//! Reports the filesystem writes a lease attempts outside what its mounts
//! allow.
//!
//! The kernel gives no record of a refused write, so the sandbox's first
//! process (the worker binary again, as `--observe`) installs a seccomp
//! user-notification filter on the payload. The filter traps write-intent
//! filesystem calls; the supervisor reads the path out of the caller, sends
//! it to the host as a datagram, and answers "continue", so the call behaves
//! exactly as it would have. The host compares the path with what the lease
//! may write and forwards the rest as denials.
//!
//! This reports attempts, not refusals: the host never sees the mount table's
//! verdict, only the paths the mounts leave writable. Paths are read from
//! memory the payload controls, so they are a diagnostic hint and nothing
//! should be enforced from them. Everything fails open: an old kernel, an
//! unreachable socket, a full socket buffer or an unreadable path only means
//! fewer events. Reads are not observed.

mod constants;
mod guest;
mod trap;

use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram as StdDatagram;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use nix::fcntl::OFlag;
use serde::{Deserialize, Serialize};
use tokio::net::UnixDatagram;
use tokio::task::JoinHandle;
use trogon_sandbox_contract::FsOperation;
use trogon_sandbox_contract::{LiteralPath, LiteralPathError};
use trogon_sandbox_protocol::LiteralPathExt;

pub use guest::{ObserveArgs, is_observe_invocation, run};

use super::mount_plan::{MountAccess, MountOp, MountPlan};
use super::relay::open_path;
use super::seccomp::SeccompOptions;
use crate::fs_violation::{FsDenial, FsDenialSink};
use crate::launch::LaunchSpec;

pub(super) use constants::GUEST_OBSERVER_BINARY;
pub use constants::OBSERVE_FLAG;
use constants::{GUEST_EVENT_SOCKET, MAX_EVENT_BYTES};

#[derive(Debug, Serialize, Deserialize)]
struct WireEvent {
    operation: FsOperation,
    path: String,
}

/// Which guest paths a lease can write. The most specific mount covering a
/// path decides, so a read-only bind inside a writable tree stays read-only.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WriteMap {
    rules: Vec<(LiteralPath, bool)>,
}

impl WriteMap {
    pub fn from_mounts(plan: &MountPlan) -> Result<Self, LiteralPathError> {
        let mut map = Self::base(plan.tmp_writable)?;
        for op in &plan.ops {
            match op {
                MountOp::Bind { dest, access, .. } => map.rules.push((dest.clone(), *access == MountAccess::ReadWrite)),
                MountOp::EmptyDir { dest }
                | MountOp::FileStub { dest }
                | MountOp::Symlink { dest, .. }
                | MountOp::RemountReadOnly { dest } => map.rules.push((dest.clone(), false)),
            }
        }
        Ok(map)
    }

    pub fn from_layout(spec: &LaunchSpec) -> anyhow::Result<Self> {
        let mut map = Self::base(true)?;
        let mut push = |sandbox: &Path, readonly: bool| -> anyhow::Result<()> {
            let raw = sandbox.to_string_lossy();
            let path = LiteralPath::new(raw.as_ref())
                .with_context(|| format!("sandbox path `{raw}` is not a valid guest path"))?;
            map.rules.push((path, !readonly));
            Ok(())
        };
        push(&spec.workspace_sandbox_path, spec.workspace_readonly)?;
        for bind in &spec.extra_binds {
            push(Path::new(&bind.sandbox_path), bind.readonly)?;
        }
        Ok(map)
    }

    fn base(tmp_writable: bool) -> Result<Self, LiteralPathError> {
        Ok(Self {
            rules: vec![
                (LiteralPath::new("/tmp")?, tmp_writable),
                (LiteralPath::new("/dev")?, true),
                (LiteralPath::new("/dev/shm")?, true),
                (LiteralPath::new("/proc")?, true),
            ],
        })
    }

    pub fn permits(&self, path: &LiteralPath) -> bool {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, (root, _))| path.is_within(root))
            .max_by_key(|(order, (root, _))| (root.as_str().len(), *order))
            .is_some_and(|(_, (_, writable))| *writable)
    }

    fn denial(&self, datagram: &[u8]) -> Option<FsDenial> {
        let event: WireEvent = serde_json::from_slice(datagram).ok()?;
        let path = LiteralPath::new(&event.path).ok()?;
        (!self.permits(&path)).then_some(FsDenial {
            path,
            operation: event.operation,
        })
    }
}

struct Collector {
    writes: WriteMap,
    sink: FsDenialSink,
}

impl Collector {
    fn accept(&self, datagram: &[u8]) {
        if let Some(denial) = self.writes.denial(datagram) {
            let _ = self.sink.send(denial);
        }
    }
}

/// The host objects an observed launch mounts into the sandbox, opened once
/// so what is mounted is what was created.
pub(super) struct ObserverWiring {
    binary: OwnedFd,
    socket: OwnedFd,
    guard: Option<ObserverGuard>,
}

impl ObserverWiring {
    /// Must be called from inside a tokio runtime.
    pub(super) fn start(binary: &Path, writes: WriteMap, sink: FsDenialSink) -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("sandbox-observe-")
            .tempdir()
            .context("creating the observer directory")?;
        let path = dir.path().join("events.sock");
        let socket = StdDatagram::bind(&path).context("binding the observer socket")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        socket.set_nonblocking(true)?;
        let drain = socket.try_clone()?;
        let socket_fd = open_path(&path, OFlag::O_NOFOLLOW)?;
        let binary_fd = open_path(binary, OFlag::empty())?;
        anyhow::ensure!(
            std::fs::metadata(proc_fd(&binary_fd))?.is_file(),
            "observer binary {} is not a regular file",
            binary.display()
        );

        let collector = Arc::new(Collector { writes, sink });
        let task = {
            let collector = Arc::clone(&collector);
            let socket = UnixDatagram::from_std(socket)?;
            tokio::spawn(async move {
                let mut buffer = vec![0u8; MAX_EVENT_BYTES];
                while let Ok(read) = socket.recv(&mut buffer).await {
                    collector.accept(&buffer[..read]);
                }
            })
        };
        Ok(Self {
            binary: binary_fd,
            socket: socket_fd,
            guard: Some(ObserverGuard {
                task,
                drain,
                collector,
                _dir: dir,
            }),
        })
    }

    pub(super) fn mount_args(&self) -> Vec<String> {
        vec![
            "--ro-bind-fd".to_string(),
            self.binary.as_raw_fd().to_string(),
            GUEST_OBSERVER_BINARY.to_string(),
            "--ro-bind-fd".to_string(),
            self.socket.as_raw_fd().to_string(),
            GUEST_EVENT_SOCKET.to_string(),
        ]
    }

    pub(super) fn inherited_fds(&self) -> [i32; 2] {
        [self.binary.as_raw_fd(), self.socket.as_raw_fd()]
    }

    pub(super) fn take_guard(&mut self) -> Option<ObserverGuard> {
        self.guard.take()
    }

    /// Runs `binary args` behind the observer. With `apply_filters` the
    /// observer installs the sandbox seccomp filters itself, which is how
    /// they reach a payload when bubblewrap is not given them.
    pub(super) fn wrap_command(
        binary: &Path,
        args: &[String],
        apply_filters: bool,
        seccomp: SeccompOptions,
    ) -> (String, Vec<String>) {
        let mut wrapped = vec![
            OBSERVE_FLAG.to_string(),
            "--socket".to_string(),
            GUEST_EVENT_SOCKET.to_string(),
        ];
        if apply_filters {
            wrapped.push("--apply-filters".to_string());
            wrapped.extend(seccomp.flags());
        }
        wrapped.push("--".to_string());
        wrapped.push(binary.to_string_lossy().into_owned());
        wrapped.extend(args.iter().cloned());
        (GUEST_OBSERVER_BINARY.to_string(), wrapped)
    }
}

fn proc_fd(fd: &OwnedFd) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()))
}

/// Stops listening when the launch ends, after taking whatever the sandbox
/// sent before it exited.
pub(super) struct ObserverGuard {
    task: JoinHandle<()>,
    drain: StdDatagram,
    collector: Arc<Collector>,
    _dir: tempfile::TempDir,
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        self.task.abort();
        let mut buffer = vec![0u8; MAX_EVENT_BYTES];
        while let Ok(read) = self.drain.recv(&mut buffer) {
            self.collector.accept(&buffer[..read]);
        }
    }
}

#[cfg(test)]
mod tests;
