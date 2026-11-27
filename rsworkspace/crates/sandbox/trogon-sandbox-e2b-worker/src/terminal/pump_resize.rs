use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use trogon_sandbox_contract::TerminalDimension;

use super::constants::TERMINAL_USER;
use super::pty_process::PtyProcess;

/// Applies the hub's resizes to the PTY in order.
pub(super) async fn pump_resize(
    process: PtyProcess,
    mut sizes: mpsc::Receiver<(TerminalDimension, TerminalDimension)>,
    done: CancellationToken,
) {
    loop {
        let size = tokio::select! {
            biased;
            _ = done.cancelled() => return,
            size = sizes.recv() => size,
        };
        let Some((cols, rows)) = size else {
            return;
        };
        if let Err(error) = process
            .envd
            .resize_pty(
                &process.target,
                TERMINAL_USER,
                &process.selector,
                cols.get(),
                rows.get(),
            )
            .await
        {
            debug!(error = %error, "terminal resize was refused");
        }
    }
}
