use tokio::io::{AsyncWriteExt, DuplexStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_ssh_gateway::GuestError;

use super::TerminalError;
use super::pty_process::PtyProcess;
use crate::envd::{PtyEvent, PtyStream};

/// Copies PTY data to the hub until the process ends or the terminal is closed. Closing kills the process; the pipe
/// is dropped on every path so the hub sees end of output.
pub(super) async fn pump_output(
    process: PtyProcess,
    mut stream: PtyStream,
    mut out: DuplexStream,
    close: CancellationToken,
    done: CancellationToken,
    exit: oneshot::Sender<Result<i32, GuestError>>,
) {
    let result = loop {
        tokio::select! {
            biased;
            _ = close.cancelled() => {
                process.kill().await;
                break Err(TerminalError::Closed);
            }
            step = step(&mut stream, &mut out) => match step {
                Ok(None) => {}
                Ok(Some(code)) => break Ok(code),
                Err(error) => break Err(error),
            },
        }
    };
    drop(out);
    done.cancel();
    exit.send(result.map_err(|error| GuestError::Unavailable(Box::new(error))))
        .ok();
}

/// Handles one stream event: `Some(code)` when the process ended.
async fn step(stream: &mut PtyStream, out: &mut DuplexStream) -> Result<Option<i32>, TerminalError> {
    match stream.next().await? {
        Some(PtyEvent::Data(bytes)) => {
            out.write_all(&bytes).await.map_err(TerminalError::Output)?;
            Ok(None)
        }
        Some(PtyEvent::Ended(code)) => Ok(Some(code)),
        Some(PtyEvent::Started(_)) => Ok(None),
        None => Err(TerminalError::StreamEnded),
    }
}
