//! Client-side request helper: encode a typed request, send it with a
//! timeout, and decode the reply per the ADR 0016 §3 error-channel rule.

use std::time::Duration;

use async_nats::HeaderMap;
use bytes::Bytes;
use thiserror::Error;
use trogon_nats::RequestClient;

use crate::binding::EndpointBinding;
use crate::constants::HEADER_CONTENT_TYPE;
use crate::content_type::{ContentType, EncodeError};
use crate::status_codec::{ReplyError, decode_reply};

/// `Transport` keeps the client's own error type rather than a rendered
/// string, so a caller can match on the concrete failure (`no responders`,
/// connection lost) instead of parsing a message.
#[derive(Debug, Error)]
pub enum RequestError<E>
where
    E: std::error::Error + 'static,
{
    #[error("failed to encode request payload")]
    Encode(#[source] EncodeError),
    #[error("NATS request to {subject} timed out")]
    Timeout { subject: String },
    #[error("NATS request to {subject} failed")]
    Transport {
        subject: String,
        #[source]
        error: E,
    },
    #[error(transparent)]
    Reply(#[from] ReplyError),
}

/// Send `request` to `endpoint`'s subject and decode the reply, per ADR 0016.
///
/// Mirrors `jsonrpc_request_with_timeout`'s shape: the timeout covers the
/// full round trip, and the response is decoded through the micro
/// error-channel rule (§3) rather than inferred from body shape.
pub async fn request<N, Req, Resp>(
    client: &N,
    endpoint: &EndpointBinding,
    content_type: ContentType,
    request: &Req,
    timeout: Duration,
) -> Result<Resp, RequestError<N::RequestError>>
where
    N: RequestClient,
    N::RequestError: 'static,
    Req: buffa::Message + serde::Serialize,
    Resp: buffa::Message + serde::de::DeserializeOwned,
{
    let subject = endpoint.subject().as_str().to_string();
    let body = content_type.encode(request).map_err(RequestError::Encode)?;

    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, content_type.header_value());

    let response = tokio::time::timeout(
        timeout,
        client.request_with_headers(subject.clone(), headers, Bytes::from(body)),
    )
    .await
    .map_err(|_| RequestError::Timeout {
        subject: subject.clone(),
    })?
    .map_err(|error| RequestError::Transport {
        subject: subject.clone(),
        error,
    })?;

    decode_reply(response.headers.as_ref(), &response.payload, content_type).map_err(RequestError::from)
}
