use agent_client_protocol::{Client, ReadTextFileRequest};
use tracing::instrument;

#[instrument(name = "acp.client.fs.read_text_file", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
    max_payload_bytes: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: ReadTextFileRequest = serde_json::from_slice(payload)?;
    let response = client.read_text_file(request).await?;
    let response = serde_json::to_vec(&response)?;
    if response.len() > max_payload_bytes {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "read_text_file response exceeds NATS max payload: {} > {}",
                response.len(),
                max_payload_bytes
            ),
        )));
    }

    Ok(response)
}
