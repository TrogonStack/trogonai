use std::any::type_name;
use std::sync::Arc;

use async_nats::Message;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::warn;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, ReadFileRequest, ReadFileResponse, WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_protocol::nats_transfer;

use crate::auth::{ClientVerb, authorize_lease, require_subject_lease};
use crate::serve::constants::{READ_FILE_PREFIX, WRITE_FILES_PREFIX};
use crate::serve::reply::respond_json;
use crate::serve::route_context::RouteContext;

/// Pulls a request body through the chunked transfer protocol and decodes it, or logs why it
/// could not.
pub(crate) async fn receive_request<Req: DeserializeOwned>(
    nats: &async_nats::Client,
    message: &Message,
) -> Option<Req> {
    let payload = match nats_transfer::receive(nats, message).await {
        Ok(payload) => payload,
        Err(error) => {
            warn!(error = %error, "file transfer failed");
            return None;
        }
    };
    match serde_json::from_slice(&payload) {
        Ok(request) => Some(request),
        Err(error) => {
            warn!(error = %error, request = type_name::<Req>(), "failed to decode request");
            None
        }
    }
}

/// A file route whose subject names the lease. The lease is authorized before a payload of up
/// to the transfer limit is pulled, and the body must then name the same lease.
struct FileTransferRoute<Req, Resp> {
    verb: ClientVerb,
    subject_prefix: &'static str,
    lease: fn(&Req) -> &LeaseId,
    denied: fn(LeaseError) -> Resp,
}

impl<Req: DeserializeOwned, Resp: Serialize> FileTransferRoute<Req, Resp> {
    async fn serve<H, Fut>(&self, context: &RouteContext, message: Message, handle: H)
    where
        H: FnOnce(Req) -> Fut,
        Fut: Future<Output = Resp>,
    {
        let Some(reply) = message.reply.clone() else {
            return;
        };
        let subject_lease = LeaseId::new(message.subject.strip_prefix(self.subject_prefix).unwrap_or_default());
        let authorized = authorize_lease(
            &context.auth,
            message.headers.as_ref(),
            self.verb,
            &subject_lease,
            context.broker.lease_tenant(&subject_lease),
        )
        .await;
        let response = match authorized {
            Err(denied) => (self.denied)(denied),
            Ok(()) => {
                let Some(request) = receive_request::<Req>(&context.nats, &message).await else {
                    return;
                };
                match require_subject_lease(&subject_lease, (self.lease)(&request)) {
                    Err(denied) => (self.denied)(denied),
                    Ok(()) => handle(request).await,
                }
            }
        };
        respond_json(&context.nats, &reply, &response).await;
    }
}

pub(crate) async fn write_files(context: Arc<RouteContext>, message: Message) {
    let route = FileTransferRoute {
        verb: ClientVerb::WriteFiles,
        subject_prefix: WRITE_FILES_PREFIX,
        lease: |request: &WriteFilesRequest| &request.lease_id,
        denied: WriteFilesResponse::Denied,
    };
    route
        .serve(&context, message, |request| context.broker.handle_write_files(request))
        .await;
}

pub(crate) async fn read_file(context: Arc<RouteContext>, message: Message) {
    let route = FileTransferRoute {
        verb: ClientVerb::ReadFile,
        subject_prefix: READ_FILE_PREFIX,
        lease: |request: &ReadFileRequest| &request.lease_id,
        denied: ReadFileResponse::Denied,
    };
    route
        .serve(&context, message, |request| context.broker.handle_read_file(request))
        .await;
}
