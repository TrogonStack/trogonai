use agent_client_protocol::{Client, WaitForTerminalExitRequest};
use tracing::instrument;

#[instrument(name = "acp.client.terminal.wait_for_exit", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: WaitForTerminalExitRequest = serde_json::from_slice(payload)?;
    let response = client.wait_for_terminal_exit(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
