use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::TerminalId;

/// One open SSH channel of a session: its terminal and the task that forwards the terminal's output.
pub(crate) struct BridgeChannel {
    pub(crate) terminal_id: TerminalId,
    pub(crate) stop_pump: CancellationToken,
}
