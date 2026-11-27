use std::os::fd::OwnedFd;

use nix::unistd::Pid;
use tokio::io::unix::AsyncFd;
use tokio_util::sync::CancellationToken;

/// One open terminal: the master side of its pseudo-terminal and the handles that end it.
pub(crate) struct TerminalSession {
    pub(crate) master: AsyncFd<OwnedFd>,
    pub(crate) group: Option<Pid>,
    /// Serializes input writes so two requests cannot interleave bytes.
    pub(crate) input: tokio::sync::Mutex<()>,
    pub(crate) stop: CancellationToken,
}
