use agent_client_protocol::{Client, ReadTextFileRequest};
use tracing::instrument;

#[instrument(name = "acp.client.fs.read_text_file", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: ReadTextFileRequest = serde_json::from_slice(payload)?;
    let response = client.read_text_file(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
