use std::sync::Arc;

use async_nats::Message;
use trogon_sandbox_contract::{FileOpRequest, FileOpResponse};

use super::file_transfer::receive_request;
use crate::auth::{ClientVerb, authorize_lease};
use crate::serve::reply::respond_json;
use crate::serve::route_context::RouteContext;

/// Answers a file operation. Its body travels through the chunked transfer protocol and names
/// the lease, so the body is pulled before the lease is authorized.
pub(crate) async fn handle(context: Arc<RouteContext>, message: Message) {
    let Some(reply) = message.reply.clone() else {
        return;
    };
    let Some(request) = receive_request::<FileOpRequest>(&context.nats, &message).await else {
        return;
    };
    let authorized = authorize_lease(
        &context.auth,
        message.headers.as_ref(),
        ClientVerb::FileOp,
        &request.lease_id,
        context.broker.lease_tenant(&request.lease_id),
    )
    .await;
    let response = match authorized {
        Err(denied) => FileOpResponse::Denied(denied),
        Ok(()) => context.broker.handle_file_op(request).await,
    };
    respond_json(&context.nats, &reply, &response).await;
}
