use agent_client_protocol::{Client, KillTerminalCommandRequest};
use tracing::instrument;

#[instrument(name = "acp.client.terminal.kill", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: KillTerminalCommandRequest = serde_json::from_slice(payload)?;
    let response = client.kill_terminal_command(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
