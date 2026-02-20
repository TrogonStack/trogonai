use agent_client_protocol::{Client, TerminalOutputRequest};
use tracing::instrument;

#[instrument(name = "acp.client.terminal.output", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: TerminalOutputRequest = serde_json::from_slice(payload)?;
    let response = client.terminal_output(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
