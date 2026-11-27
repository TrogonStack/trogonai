use tokio::io::{AsyncReadExt, DuplexStream};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::constants::{INPUT_CHUNK_BYTES, TERMINAL_USER};
use super::pty_process::PtyProcess;

/// Sends the hub's input to the PTY in order.
pub(super) async fn pump_input(process: PtyProcess, mut input: DuplexStream, done: CancellationToken) {
    let mut buf = vec![0u8; INPUT_CHUNK_BYTES];
    loop {
        let read = tokio::select! {
            biased;
            _ = done.cancelled() => return,
            read = input.read(&mut buf) => read,
        };
        let n = match read {
            Ok(n) if n > 0 => n,
            _ => return,
        };
        if let Err(error) = process
            .envd
            .send_pty_input(&process.target, TERMINAL_USER, &process.selector, &buf[..n])
            .await
        {
            // Dropping the pipe makes the hub's next write fail.
            debug!(error = %error, "terminal input was refused");
            return;
        }
    }
}
