use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError};

use super::constants::{PIPE_BYTES, RESIZE_QUEUE};
use super::pty_process::PtyProcess;
use super::pump_input::pump_input;
use super::pump_output::pump_output;
use super::pump_resize::pump_resize;
use crate::envd::PtyStream;

/// Wires a started PTY stream to the pipes and tasks of a [`GuestChannel`].
pub(super) fn spawn_pty(process: PtyProcess, stream: PtyStream) -> GuestChannel {
    let (out_write, out_read) = tokio::io::duplex(PIPE_BYTES);
    let (in_write, in_read) = tokio::io::duplex(PIPE_BYTES);
    let (resize_tx, resize_rx) = mpsc::channel(RESIZE_QUEUE);
    let (exit_tx, exit_rx) = oneshot::channel();
    let close = CancellationToken::new();
    // Cancelled when the stream is over, which retires the input and resize tasks.
    let done = CancellationToken::new();

    tokio::spawn(pump_output(
        process.clone(),
        stream,
        out_write,
        close.clone(),
        done.clone(),
        exit_tx,
    ));
    tokio::spawn(pump_input(process.clone(), in_read, done.clone()));
    tokio::spawn(pump_resize(process, resize_rx, done));

    GuestChannel {
        stdin: Box::pin(in_write),
        stdout: Box::pin(out_read),
        resize: resize_tx,
        completion: Box::pin(async move { exit_rx.await.unwrap_or(Err(GuestError::NoExitStatus)) }),
        close,
    }
}
