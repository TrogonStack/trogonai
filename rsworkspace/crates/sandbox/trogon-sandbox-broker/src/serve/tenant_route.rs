use std::sync::Arc;

use async_nats::Message;
use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_sandbox_contract::TenantId;

use super::on_decode_error::OnDecodeError;
use super::reply::publish_json;
use super::route_context::RouteContext;
use crate::auth::{ClientVerb, Unauthorized, authorize_tenant};

/// A request-reply route whose request names the tenant it acts for.
pub(crate) struct TenantRoute<Req, Resp> {
    verb: ClientVerb,
    tenant: fn(&Req) -> &TenantId,
    denied: fn(Unauthorized) -> Resp,
    on_decode_error: OnDecodeError<Resp>,
}

impl<Req, Resp> TenantRoute<Req, Resp>
where
    Req: DeserializeOwned,
    Resp: Serialize,
{
    /// A route that logs and drops a request body it cannot decode.
    pub(crate) fn new(verb: ClientVerb, tenant: fn(&Req) -> &TenantId, denied: fn(Unauthorized) -> Resp) -> Self {
        Self {
            verb,
            tenant,
            denied,
            on_decode_error: OnDecodeError::Warn,
        }
    }

    pub(crate) fn on_decode_error(self, on_decode_error: OnDecodeError<Resp>) -> Self {
        Self {
            on_decode_error,
            ..self
        }
    }

    /// Decodes, authorizes and answers one message. Messages without a reply subject are dropped.
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
        let decision = authorize_tenant(
            &context.auth,
            message.headers.as_ref(),
            self.verb,
            (self.tenant)(&request),
        )
        .await;
        let response = match decision {
            Err(denied) => (self.denied)(denied),
            Ok(()) => handle(Arc::clone(&context), request).await,
        };
        publish_json(&context.nats, reply, &response).await;
    }
}
