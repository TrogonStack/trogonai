//! Registers a [`ServiceBinding`] as a NATS micro service (ADR 0016 §1, §5):
//! discovery, versioning, and per-endpoint stats come from `async_nats`'s
//! `service` feature. Only the error reply path (§3) bypasses micro's own
//! `respond`, because micro's error-respond helper cannot carry a body (see
//! [`reply_error`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_nats::service::ServiceExt as _;
use async_nats::{Client, HeaderMap};
use futures_util::StreamExt as _;
use thiserror::Error;
use trogon_nats::PublishClient;
use trogonai_proto::nats::micro::v1alpha1::ServiceOptions;

use crate::binding::ServiceBinding;
use crate::constants::HEADER_CONTENT_TYPE;
use crate::content_type::{ContentType, EncodeError};
use crate::content_type_input::ContentTypeInput;
use crate::service_fault::ServiceFault;
use crate::status_codec::{self, Outcome};

/// Decodes a request payload and produces a reply payload for one endpoint.
///
/// The handler receives the request bytes already isolated from NATS
/// transport concerns and returns the success reply body pre-encoded in
/// `content_type`, or a [`ServiceFault`] to report on the micro error
/// channel.
/// Pre-encoding the success body here (rather than a typed message) keeps
/// this trait's signature independent of any one request/response message
/// pair, so one registration loop can dispatch to endpoints with unrelated
/// message types. The trait is boxed-future based (rather than `impl
/// Future`) so `Box<dyn EndpointHandler>` values with different concrete
/// request/response types can share one `Vec` in [`serve`].
pub trait EndpointHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>>;
}

#[derive(Debug, Error)]
pub enum ServeError {
    #[error("binding declares {endpoints} endpoints but {handlers} handlers were supplied")]
    HandlerCount { endpoints: usize, handlers: usize },
    #[error("failed to start NATS micro service: {0}")]
    Start(#[source] async_nats::Error),
    #[error("failed to register endpoint {subject}: {source}")]
    Endpoint {
        subject: String,
        #[source]
        source: async_nats::Error,
    },
}

/// Serve a [`ServiceBinding`] as a NATS micro service, dispatching each
/// endpoint's requests to its handler on a dedicated task until the returned
/// [`async_nats::service::Service`] is stopped or dropped.
///
/// `content_type_policy` is the service's `ServiceOptions.content_type`
/// restriction (ADR 0016 §4); every endpoint negotiates against the same
/// policy. `handlers` must have exactly one entry per
/// `binding.endpoints()`, in the same order.
pub async fn serve(
    client: &Client,
    binding: &ServiceBinding,
    content_type_policy: ServiceOptions,
    handlers: Vec<Box<dyn EndpointHandler>>,
) -> Result<async_nats::service::Service, ServeError> {
    if binding.endpoints().len() != handlers.len() {
        return Err(ServeError::HandlerCount {
            endpoints: binding.endpoints().len(),
            handlers: handlers.len(),
        });
    }

    let mut builder = client.service_builder();
    if let Some(description) = binding.description() {
        builder = builder.description(description);
    }
    // Only when there is something to report: micro omits the field entirely
    // when it is unset, so setting an empty map would publish `metadata: {}`
    // into every discovery record that declares none.
    if !binding.metadata().is_empty() {
        builder = builder.metadata(binding.metadata().entries().clone());
    }
    let service = builder
        .start(binding.name().as_str(), binding.version().as_str())
        .await
        .map_err(ServeError::Start)?;

    let content_type_policy = Arc::new(content_type_policy);
    for (endpoint, handler) in binding.endpoints().iter().zip(handlers) {
        let subject = endpoint.subject().as_str().to_string();
        // Name the endpoint after the rpc method (ADR 0016 §2). Micro
        // otherwise derives the name from the full subject, so `$SRV.INFO`
        // and `$SRV.STATS` would report the dotted subject instead of the
        // method the binding declared.
        let mut endpoint_builder = service.endpoint_builder().name(endpoint.method_name().as_str());
        if !endpoint.metadata().is_empty() {
            endpoint_builder = endpoint_builder.metadata(endpoint.metadata().entries().clone());
        }
        let registration = endpoint_builder.add(subject.clone()).await;
        let mut micro_endpoint = registration.map_err(|source| ServeError::Endpoint { subject, source })?;

        let client = client.clone();
        let content_type_policy = content_type_policy.clone();
        tokio::spawn(async move {
            while let Some(request) = micro_endpoint.next().await {
                dispatch(&client, &request, &content_type_policy, handler.as_ref()).await;
            }
        });
    }

    Ok(service)
}

async fn dispatch<P: PublishClient>(
    client: &P,
    request: &async_nats::service::Request,
    content_type_policy: &ServiceOptions,
    handler: &dyn EndpointHandler,
) {
    let (content_type, outcome) = resolve(
        request.message.headers.as_ref(),
        &request.message.payload,
        content_type_policy,
        handler,
    )
    .await;

    match outcome {
        Ok(body) => reply_success(request, body, content_type).await,
        Err(fault) => {
            let reply = request.message.reply.clone();
            let published = reply_error(client, reply, fault, content_type).await;
            let _ = published.inspect_err(warn_unencodable);
        }
    }
}

/// Negotiate the request's encoding (ADR 0016 §4), run the handler, and report
/// the encoding the reply must use either way.
///
/// A caller the policy turns away still has to be able to read why, so the
/// rejection is reported in the encoding the caller asked for; an encoding this
/// binding does not speak leaves protobuf as the only choice.
async fn resolve(
    headers: Option<&HeaderMap>,
    payload: &[u8],
    content_type_policy: &ServiceOptions,
    handler: &dyn EndpointHandler,
) -> (ContentType, Result<Vec<u8>, ServiceFault>) {
    let requested = headers
        .and_then(|headers| headers.get(HEADER_CONTENT_TYPE))
        .map(|value| ContentTypeInput::new(value.as_str()));

    match ContentType::negotiate(content_type_policy, requested.as_ref()) {
        Ok(content_type) => {
            let outcome = handler.handle(payload, content_type).await;
            (content_type, outcome)
        }
        Err(error) => {
            let rejection_content_type = requested
                .as_ref()
                .and_then(ContentType::from_input)
                .unwrap_or(ContentType::Protobuf);
            (
                rejection_content_type,
                Err(ServiceFault::invalid_argument(error.to_string())),
            )
        }
    }
}

async fn reply_success(request: &async_nats::service::Request, body: Vec<u8>, content_type: ContentType) {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, content_type.header_value());
    let published = request
        .respond_with_headers(Ok(bytes::Bytes::from(body)), headers)
        .await;
    warn_if_undelivered(published, ReplyKind::Success);
}

/// Publish an error reply directly on the client, bypassing
/// [`async_nats::service::Request::respond_with_headers`].
///
/// `respond_with_headers` always publishes an empty body on `Err(..)`
/// (`async-nats` 0.49.1 `service/mod.rs`), which cannot satisfy ADR 0016 §3:
/// the error reply body must be one complete `google.rpc.Status`. Publishing
/// directly is the only way to set both the error headers and a non-empty
/// body. The trade-off is that this bypasses micro's own `num_errors` /
/// `last_error` endpoint statistics bookkeeping, which only `respond`/
/// `respond_with_headers` update; ADR 0016 treats stats-counting as a
/// convenience micro provides, not an invariant, so body-completeness wins.
async fn reply_error<P: PublishClient>(
    client: &P,
    reply: Option<async_nats::Subject>,
    fault: ServiceFault,
    content_type: ContentType,
) -> Result<(), EncodeError> {
    let Some(reply) = reply else {
        tracing::warn!("grpc-nats-micro: request had no reply subject; dropping error reply");
        return Ok(());
    };
    let encoded = status_codec::encode_reply(Outcome::Error(fault), content_type)?;
    let mut headers = encoded.headers;
    headers.insert(HEADER_CONTENT_TYPE, content_type.header_value());
    let published = client.publish_with_headers(reply, headers, encoded.body).await;
    warn_if_undelivered(published, ReplyKind::Error);
    Ok(())
}

/// A `Status` that will not encode leaves nothing to report: ADR 0016 §3 makes
/// the error body one complete `Status`, and half of one is not that.
fn warn_unencodable(error: &EncodeError) {
    tracing::warn!(error = %error, "grpc-nats-micro: failed to encode error reply");
}

/// Which half of the reply contract a failed publish belongs to, so the log
/// line says what was lost without a second message per call site.
enum ReplyKind {
    Success,
    Error,
}

impl ReplyKind {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
        }
    }
}

/// A reply that cannot be delivered has nowhere left to go: micro offers no
/// redelivery channel, and the caller learns of it by timing out.
fn warn_if_undelivered<E>(published: Result<(), E>, reply_kind: ReplyKind)
where
    E: std::fmt::Display,
{
    if let Err(error) = published {
        let reply_kind = reply_kind.as_str();
        tracing::warn!(error = %error, reply_kind, "grpc-nats-micro: failed to publish reply");
    }
}

#[cfg(test)]
mod tests;
