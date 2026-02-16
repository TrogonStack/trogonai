use agent_client_protocol::{Client, RequestPermissionRequest};
use tracing::instrument;

#[instrument(name = "acp.client.session.request_permission", skip(payload, client))]
pub async fn handle<C: Client>(
    payload: &[u8],
    client: &C,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let request: RequestPermissionRequest = serde_json::from_slice(payload)?;
    let response = client.request_permission(request).await?;
    Ok(serde_json::to_vec(&response)?)
}
