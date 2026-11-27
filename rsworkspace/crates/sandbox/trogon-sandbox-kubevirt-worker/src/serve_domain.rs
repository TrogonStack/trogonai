use std::sync::Arc;

use async_nats::{Client, Message};
use bytes::Bytes;
use trogon_sandbox_contract::{GetSandboxDomainRequest, GetSandboxDomainResponse};
use trogon_sandbox_protocol::nats_transfer;

use crate::{LeaseHolding, ServeDomainError};

/// Answers one get_domain request. A lease `holder` does not hold gets no answer, so the worker that holds it can
/// answer; a held lease gets [`GetSandboxDomainResponse::Unsupported`], because VMIs expose no per-lease ingress yet.
pub async fn serve_domain(
    client: Client,
    holder: Arc<dyn LeaseHolding>,
    message: Message,
) -> Result<(), ServeDomainError> {
    let Some(reply) = message.reply.clone() else {
        return Ok(());
    };
    let payload = nats_transfer::receive(&client, &message).await?;
    let request: GetSandboxDomainRequest = serde_json::from_slice(&payload)?;
    if !holder.holds(&request.lease_id).await {
        return Ok(());
    }
    let body = serde_json::to_vec(&GetSandboxDomainResponse::Unsupported)?;
    nats_transfer::respond(&client, reply.as_str(), Bytes::from(body)).await?;
    Ok(())
}
