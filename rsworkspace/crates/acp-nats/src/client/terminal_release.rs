use agent_client_protocol::{Client, ReleaseTerminalRequest};
use tracing::instrument;

#[instrument(name = "acp.client.terminal.release", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: ReleaseTerminalRequest = serde_json::from_slice(payload)?;
    let response = client.release_terminal(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
