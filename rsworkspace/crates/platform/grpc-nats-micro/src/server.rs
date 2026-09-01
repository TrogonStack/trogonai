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
use buffa::Enumeration as _;
use futures_util::StreamExt as _;
use thiserror::Error;
use trogonai_proto::google::rpc::{Code, Status};
use trogonai_proto::nats::micro::v1alpha1::ServiceOptions;

use crate::binding::ServiceBinding;
use crate::constants::HEADER_CONTENT_TYPE;
use crate::content_type::ContentType;
use crate::status_codec::{self, Outcome};

/// Decodes a request payload and produces a reply payload for one endpoint.
///
/// The handler receives the request bytes already isolated from NATS
/// transport concerns and returns the success reply body pre-encoded in
/// `content_type`, or a [`Status`] to report on the micro error channel.
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, Status>> + Send + 'a>>;
}

#[derive(Debug, Error)]
pub enum ServeError {
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
    let mut builder = client.service_builder();
    if let Some(description) = binding.description() {
        builder = builder.description(description);
    }
    let service = builder
        .start(binding.name(), binding.version())
        .await
        .map_err(ServeError::Start)?;

    let content_type_policy = Arc::new(content_type_policy);
    for (endpoint, handler) in binding.endpoints().iter().zip(handlers) {
        let subject = endpoint.subject().as_str().to_string();
        let micro_endpoint = service
            .endpoint(subject.clone())
            .await
            .map_err(|source| ServeError::Endpoint {
                subject: subject.clone(),
                source,
            })?;

        tokio::spawn(run_endpoint(
            client.clone(),
            micro_endpoint,
            content_type_policy.clone(),
            handler,
        ));
    }

    Ok(service)
}

async fn run_endpoint(
    client: Client,
    mut micro_endpoint: async_nats::service::endpoint::Endpoint,
    content_type_policy: Arc<ServiceOptions>,
    handler: Box<dyn EndpointHandler>,
) {
    while let Some(request) = micro_endpoint.next().await {
        dispatch(&client, &request, &content_type_policy, handler.as_ref()).await;
    }
}

async fn dispatch(
    client: &Client,
    request: &async_nats::service::Request,
    content_type_policy: &ServiceOptions,
    handler: &dyn EndpointHandler,
) {
    let header_value = request
        .message
        .headers
        .as_ref()
        .and_then(|headers| headers.get(HEADER_CONTENT_TYPE))
        .map(|value| value.as_str());

    let content_type = match ContentType::negotiate(content_type_policy, header_value) {
        Ok(content_type) => content_type,
        Err(error) => {
            let status = Status {
                code: Code::INVALID_ARGUMENT.to_i32(),
                message: error.to_string(),
                details: Vec::new(),
            };
            reply_error(client, request, status, ContentType::Protobuf).await;
            return;
        }
    };

    match handler.handle(&request.message.payload, content_type).await {
        Ok(body) => reply_success(request, body, content_type).await,
        Err(status) => reply_error(client, request, status, content_type).await,
    }
}

async fn reply_success(request: &async_nats::service::Request, body: Vec<u8>, content_type: ContentType) {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, content_type.header_value());
    if let Err(source) = request
        .respond_with_headers(Ok(bytes::Bytes::from(body)), headers)
        .await
    {
        tracing::warn!(error = %source, "grpc-nats-micro: failed to publish success reply");
    }
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
async fn reply_error(
    client: &Client,
    request: &async_nats::service::Request,
    status: Status,
    content_type: ContentType,
) {
    let Some(reply) = request.message.reply.clone() else {
        tracing::warn!("grpc-nats-micro: request had no reply subject; dropping error reply");
        return;
    };
    let encoded = match status_codec::encode_reply(Outcome::Error(status), content_type) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::warn!(error = %error, "grpc-nats-micro: failed to encode error reply");
            return;
        }
    };
    let mut headers = encoded.headers;
    headers.insert(HEADER_CONTENT_TYPE, content_type.header_value());
    if let Err(source) = client.publish_with_headers(reply, headers, encoded.body).await {
        tracing::warn!(error = %source, "grpc-nats-micro: failed to publish error reply");
    }
}
