use agent_client_protocol::{Client, CreateTerminalRequest};
use tracing::instrument;

#[instrument(name = "acp.client.terminal.create", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: CreateTerminalRequest = serde_json::from_slice(payload)?;
    let response = client.create_terminal(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
