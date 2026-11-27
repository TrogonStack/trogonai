use std::future::Future;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::TerminalDimension;

use crate::GuestError;

/// One terminal's streams into the guest.
pub struct GuestChannel {
    /// Bytes written here reach the PTY.
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    /// What the PTY prints.
    pub stdout: Pin<Box<dyn AsyncRead + Send>>,
    /// New `(cols, rows)` sizes for the PTY.
    pub resize: mpsc::Sender<(TerminalDimension, TerminalDimension)>,
    /// The command's exit status, once known.
    pub completion: Pin<Box<dyn Future<Output = Result<i32, GuestError>> + Send>>,
    /// Cancelled to close the channel and hang up the PTY.
    pub close: CancellationToken,
}
