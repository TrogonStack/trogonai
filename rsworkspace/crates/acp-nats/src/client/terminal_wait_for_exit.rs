use agent_client_protocol::{Client, WaitForTerminalExitRequest};
use std::time::Duration;
use tracing::instrument;
use tokio::time::timeout;

#[instrument(name = "acp.client.terminal.wait_for_exit", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
    operation_timeout: Duration,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: WaitForTerminalExitRequest = serde_json::from_slice(payload)?;
    let response = timeout(operation_timeout, client.wait_for_terminal_exit(request))
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Timed out waiting for terminal exit",
            )
        })?
        .map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, format!("Terminal wait_for_exit failed: {}", e))
        })?;

    Ok(serde_json::to_vec(&response)?)
}
