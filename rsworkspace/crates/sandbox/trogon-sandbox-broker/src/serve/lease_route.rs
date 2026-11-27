use std::sync::Arc;

use async_nats::Message;
use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_sandbox_contract::{LeaseError, LeaseId};

use super::on_decode_error::OnDecodeError;
use super::reply::publish_json;
use super::route_context::RouteContext;
use crate::auth::{ClientVerb, authorize_lease};

/// A request-reply route whose request names the lease it acts on.
pub(crate) struct LeaseRoute<Req, Resp> {
    verb: ClientVerb,
    lease: fn(&Req) -> &LeaseId,
    denied: fn(LeaseError) -> Resp,
    on_decode_error: OnDecodeError<Resp>,
}

impl<Req, Resp> LeaseRoute<Req, Resp>
where
    Req: DeserializeOwned,
    Resp: Serialize,
{
    /// A route that logs and drops a request body it cannot decode.
    pub(crate) fn new(verb: ClientVerb, lease: fn(&Req) -> &LeaseId, denied: fn(LeaseError) -> Resp) -> Self {
        Self {
            verb,
            lease,
            denied,
            on_decode_error: OnDecodeError::Warn,
        }
    }

    /// A route that answers an undecodable body with its own denial.
    pub(crate) fn replying_to_decode_errors(self) -> Self {
        Self {
            on_decode_error: OnDecodeError::Reply(self.denied),
            ..self
        }
    }

    /// Decodes, authorizes against the lease owner and answers one message. Messages without a
    /// reply subject are dropped.
    pub(crate) async fn serve<H, Fut>(&self, context: Arc<RouteContext>, message: Message, handle: &H)
    where
        H: Fn(Arc<RouteContext>, Req) -> Fut,
        Fut: Future<Output = Resp>,
    {
        let Some(reply) = message.reply.clone() else {
            return;
        };
        let request: Req = match serde_json::from_slice(&message.payload) {
            Ok(request) => request,
            Err(error) => {
                if let Some(response) = self.on_decode_error.response::<Req>(&error) {
                    publish_json(&context.nats, reply, &response).await;
                }
                return;
            }
        };
        let lease_id = (self.lease)(&request);
        let decision = authorize_lease(
            &context.auth,
            message.headers.as_ref(),
            self.verb,
            lease_id,
            context.broker.lease_tenant(lease_id),
        )
        .await;
        let response = match decision {
            Err(denied) => (self.denied)(denied),
            Ok(()) => handle(Arc::clone(&context), request).await,
        };
        publish_json(&context.nats, reply, &response).await;
    }
}
