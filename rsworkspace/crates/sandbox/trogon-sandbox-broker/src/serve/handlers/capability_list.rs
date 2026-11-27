use std::sync::Arc;

use async_nats::Message;
use tracing::warn;
use trogon_sandbox_contract::ListCapabilitiesRequest;

use crate::serve::reply::publish_json;
use crate::serve::route_context::RouteContext;

/// Reads a capability listing request. An empty body asks for the default listing.
pub(crate) fn capability_request(payload: &[u8]) -> Option<ListCapabilitiesRequest> {
    if payload.is_empty() {
        return Some(ListCapabilitiesRequest::default());
    }
    match serde_json::from_slice(payload) {
        Ok(request) => Some(request),
        Err(error) => {
            warn!(error = %error, request = "ListCapabilitiesRequest", "failed to decode request");
            None
        }
    }
}

/// Lists provider capabilities. The listing is not tenant scoped, so it is not authorized.
pub(crate) async fn handle(context: Arc<RouteContext>, message: Message) {
    let Some(reply) = message.reply.clone() else {
        return;
    };
    let Some(request) = capability_request(&message.payload) else {
        return;
    };
    let response = context.broker.handle_list_capabilities(request).await;
    publish_json(&context.nats, reply, &response).await;
}

#[cfg(test)]
mod tests;
