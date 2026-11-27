use russh::client;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::TerminalDimension;

use crate::constants::{PIPE_BYTES, RESIZE_QUEUE};
use crate::pty_pump::pump;
use crate::{GuestChannel, GuestError};

/// Requests a PTY of `size` (`cols`, `rows`) on `channel`, starts `command` (or the login shell when `None`), and
/// returns the terminal's streams.
pub async fn start_pty_channel(
    channel: russh::Channel<client::Msg>,
    size: (TerminalDimension, TerminalDimension),
    term: &str,
    command: Option<String>,
) -> Result<GuestChannel, GuestError> {
    channel
        .request_pty(false, term, u32::from(size.0.get()), u32::from(size.1.get()), 0, 0, &[])
        .await?;
    match &command {
        Some(line) => channel.exec(false, line.as_bytes()).await?,
        None => channel.request_shell(false).await?,
    }

    let (stdin, stdin_rx) = tokio::io::duplex(PIPE_BYTES);
    let (stdout_tx, stdout) = tokio::io::duplex(PIPE_BYTES);
    let (resize, resize_rx) = mpsc::channel(RESIZE_QUEUE);
    let (done_tx, done_rx) = oneshot::channel();
    let close = CancellationToken::new();
    tokio::spawn(pump(channel, stdin_rx, stdout_tx, resize_rx, close.clone(), done_tx));
    Ok(GuestChannel {
        stdin: Box::pin(stdin),
        stdout: Box::pin(stdout),
        resize,
        completion: Box::pin(async move { done_rx.await.map_err(|_| GuestError::NoExitStatus) }),
        close,
    })
}
