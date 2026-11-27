//! Dialing a port inside a bubblewrap lease from the worker host.
//!
//! A bridge is the worker binary re-executed as `--port-bridge <port>` after
//! the forked child joined the lease's user and network namespaces, so its
//! `127.0.0.1` is the lease's loopback. The user namespace comes first: the
//! worker holds no capability in its own, but as the owner of the namespace
//! bubblewrap created it holds all of them there, which is what `setns` into
//! the network namespace needs. `setns` into a user namespace is refused to a
//! multithreaded process, which is why this is a child process and not a
//! thread of the worker.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::bridge_io::{BridgeIo, Connector};

mod constants;

pub use constants::BRIDGE_FLAG;
use constants::{CONNECT_TIMEOUT, CONNECTED, FAILED, MAX_DISCOVERY_DEPTH, MAX_DISCOVERY_PROCESSES, REFUSED};

pub fn is_bridge_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1).is_some_and(|arg| arg == BRIDGE_FLAG)
}

/// The helper's entry point: joins stdin/stdout to `127.0.0.1:<port>`. Returns
/// the process exit code.
pub fn run_bridge(args: &[std::ffi::OsString]) -> i32 {
    let Some(port) = args.get(2).and_then(|a| a.to_str()).and_then(|a| a.parse::<u16>().ok()) else {
        let _ = writeln!(io::stderr().lock(), "usage: {BRIDGE_FLAG} <port>");
        return 2;
    };
    let mut stdout = io::stdout().lock();
    let mut stream =
        match TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], port)), CONNECT_TIMEOUT) {
            Ok(stream) => stream,
            Err(e) => {
                let status = if e.kind() == io::ErrorKind::ConnectionRefused {
                    REFUSED
                } else {
                    FAILED
                };
                let _ = stdout.write_all(&[status]).and_then(|_| stdout.flush());
                return 1;
            }
        };
    if stdout.write_all(&[CONNECTED]).and_then(|_| stdout.flush()).is_err() {
        return 1;
    }
    let Ok(mut writer) = stream.try_clone() else {
        return 1;
    };
    // The client's end of input half-closes the connection; the reply keeps
    // draining until the server closes.
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut writer);
        let _ = writer.shutdown(Shutdown::Write);
    });
    let copied = copy_response(&mut stream, &mut stdout);
    i32::from(copied.is_err())
}

fn copy_response(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    // Blocking splice can hold the stdout pipe lock while the caller is
    // waiting to read the connection acknowledgement from that same pipe.
    let mut bytes = [0; 16_384];
    loop {
        let count = match reader.read(&mut bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return writer.flush();
        }
        writer.write_all(&bytes[..count])?;
        writer.flush()?;
    }
}

/// The namespaces of one lease, held open for as long as the lease runs.
pub struct NamespaceConnector {
    /// `None` when the lease shares the worker's user namespace.
    user: Option<OwnedFd>,
    net: OwnedFd,
    helper: PathBuf,
}

impl NamespaceConnector {
    /// Finds the lease's namespaces from `root`, the bubblewrap process, and
    /// bridges through the running executable.
    pub fn open(root: u32) -> io::Result<Self> {
        Self::open_with_helper(root, std::env::current_exe()?)
    }

    /// As [`Self::open`], bridging through `helper`, a binary that handles
    /// `--port-bridge` the way the worker does.
    pub fn open_with_helper(root: u32, helper: PathBuf) -> io::Result<Self> {
        ProcessDirectory::open(root)?.namespaces(helper)
    }
}

pub(crate) struct NamespaceTarget {
    root: ProcessDirectory,
    helper: PathBuf,
    connector: tokio::sync::OnceCell<NamespaceConnector>,
}

impl NamespaceTarget {
    pub(crate) fn open(root: u32, helper: PathBuf) -> io::Result<Self> {
        Ok(Self {
            root: ProcessDirectory::open(root)?,
            helper,
            connector: tokio::sync::OnceCell::new(),
        })
    }
}

#[async_trait]
impl Connector for NamespaceTarget {
    async fn connect(&self, port: u16) -> io::Result<Box<dyn BridgeIo>> {
        self.root.validate()?;
        self.connector
            .get_or_try_init(|| async { self.root.namespaces(self.helper.clone()) })
            .await?
            .connect(port)
            .await
    }
}

#[async_trait]
impl Connector for NamespaceConnector {
    async fn connect(&self, port: u16) -> io::Result<Box<dyn BridgeIo>> {
        let user = self.user.as_ref().map(AsRawFd::as_raw_fd);
        let net = self.net.as_raw_fd();
        let mut command = Command::new(&self.helper);
        command
            .arg(BRIDGE_FLAG)
            .arg(port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // SAFETY: runs after fork in the child and calls only `setns`, which
        // is async-signal-safe; the descriptors stay open in the parent for
        // the life of `self`.
        unsafe {
            command.pre_exec(move || {
                if let Some(user) = user
                    && libc::setns(user, libc::CLONE_NEWUSER) == -1
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::setns(net, libc::CLONE_NEWNET) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("the port bridge has no stdout"))?;
        let mut status = [FAILED];
        tokio::time::timeout(CONNECT_TIMEOUT, stdout.read_exact(&mut status))
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "port bridge did not acknowledge its connection",
                )
            })??;
        match status[0] {
            CONNECTED => {}
            REFUSED => return Err(io::ErrorKind::ConnectionRefused.into()),
            FAILED => return Err(io::Error::other("port bridge connection failed")),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid port bridge acknowledgement",
                ));
            }
        }
        Ok(Box::new(HelperIo {
            stdin,
            stdout,
            _child: child,
        }))
    }
}

/// The helper's stdin and stdout as one stream. Dropping it kills the helper.
struct HelperIo {
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
    _child: Child,
}

impl AsyncRead for HelperIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for HelperIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        match self.stdin.as_mut() {
            Some(stdin) => Pin::new(stdin).poll_write(cx, data),
            None => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.stdin.as_mut() {
            Some(stdin) => Pin::new(stdin).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    /// Closing the pipe is what hands the helper its end of input.
    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.stdin = None;
        Poll::Ready(Ok(()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProcessIdentity {
    parent: u32,
    started: u64,
}

struct ProcessDirectory {
    pid: u32,
    directory: File,
    identity: ProcessIdentity,
}

impl ProcessDirectory {
    fn open(pid: u32) -> io::Result<Self> {
        let directory = File::open(format!("/proc/{pid}"))?;
        let identity = Self::identity(&directory)?;
        Ok(Self {
            pid,
            directory,
            identity,
        })
    }

    fn path(directory: &File, suffix: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}/{suffix}", directory.as_raw_fd()))
    }

    fn identity(directory: &File) -> io::Result<ProcessIdentity> {
        let mut stat = String::new();
        File::open(Self::path(directory, "stat"))?.read_to_string(&mut stat)?;
        let after = stat
            .rsplit_once(')')
            .ok_or_else(|| io::Error::other("invalid process stat"))?
            .1;
        let fields: Vec<_> = after.split_whitespace().collect();
        if matches!(fields.first(), Some(&"Z" | &"X")) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "owned process has exited"));
        }
        let parse = || {
            Some(ProcessIdentity {
                parent: fields.get(1)?.parse().ok()?,
                started: fields.get(19)?.parse().ok()?,
            })
        };
        parse().ok_or_else(|| io::Error::other("invalid process identity"))
    }

    fn validate(&self) -> io::Result<()> {
        if Self::identity(&self.directory)? != self.identity {
            return Err(io::Error::other("owned process identity or ancestry changed"));
        }
        Ok(())
    }

    fn namespace(&self, kind: &str) -> io::Result<File> {
        File::open(Self::path(&self.directory, &format!("ns/{kind}")))
    }

    fn children(&self, deadline: Instant) -> io::Result<Vec<Arc<Self>>> {
        self.validate()?;
        let mut pids = std::collections::BTreeSet::new();
        for (count, task) in std::fs::read_dir(Self::path(&self.directory, "task"))?.enumerate() {
            if count >= MAX_DISCOVERY_PROCESSES || Instant::now() >= deadline {
                return Err(io::Error::other("owned process discovery limit reached"));
            }
            for pid in std::fs::read_to_string(task?.path().join("children"))?.split_whitespace() {
                let pid = pid
                    .parse::<u32>()
                    .map_err(|_| io::Error::other("invalid owned child identity"))?;
                pids.insert(pid);
                if pids.len() > MAX_DISCOVERY_PROCESSES {
                    return Err(io::Error::other("owned process discovery limit reached"));
                }
            }
        }
        let children = pids
            .into_iter()
            .map(|pid| {
                let child = Self::open(pid)?;
                if child.identity.parent != self.pid {
                    return Err(io::Error::other("owned child ancestry changed"));
                }
                Ok(Arc::new(child))
            })
            .collect::<io::Result<Vec<_>>>()?;
        self.validate()?;
        Ok(children)
    }

    fn namespaces(&self, helper: PathBuf) -> io::Result<NamespaceConnector> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        self.validate()?;
        let own_net = File::open("/proc/self/ns/net")?.metadata()?.ino();
        let own_user = File::open("/proc/self/ns/user")?.metadata()?.ino();
        let mut queue = std::collections::VecDeque::new();
        for child in self.children(deadline)? {
            queue.push_back(vec![child]);
        }
        let mut discovered = queue.len();
        while let Some(chain) = queue.pop_front() {
            if chain.len() > MAX_DISCOVERY_DEPTH || Instant::now() >= deadline {
                return Err(io::Error::other("owned namespace ancestry limit reached"));
            }
            let Some(process) = chain.last() else {
                continue;
            };
            let mut expected_parent = self.pid;
            for ancestor in &chain {
                ancestor.validate()?;
                if ancestor.identity.parent != expected_parent {
                    return Err(io::Error::other("owned namespace ancestry changed"));
                }
                expected_parent = ancestor.pid;
            }
            let net = process.namespace("net")?;
            if net.metadata()?.ino() != own_net {
                let user = process.namespace("user")?;
                let user = (user.metadata()?.ino() != own_user).then(|| OwnedFd::from(user));
                // Each directory pins its original process. Revalidate the entire
                // live chain after opening the namespace FDs, before granting access.
                for ancestor in chain.iter().rev() {
                    ancestor.validate()?;
                }
                self.validate()?;
                return Ok(NamespaceConnector {
                    user,
                    net: OwnedFd::from(net),
                    helper,
                });
            }
            for child in process.children(deadline)? {
                discovered += 1;
                if discovered > MAX_DISCOVERY_PROCESSES {
                    return Err(io::Error::other("owned namespace discovery limit reached"));
                }
                let mut children = chain.clone();
                children.push(child);
                queue.push_back(children);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no live owned process has a private network namespace",
        ))
    }
}

#[cfg(test)]
mod tests;
