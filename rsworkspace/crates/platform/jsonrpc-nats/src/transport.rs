//! Shared JSON-RPC over NATS transport helpers.
//!
//! Domain crates supply subject routing and encoded bodies. These helpers own
//! header merge, core request/reply, and fire-and-forget publish over the
//! canonical encoding (ADR#0056).

use std::time::Duration;

use async_nats::header::HeaderMap;
use bytes::Bytes;
use serde::Serialize;
use thiserror::Error;
use trogon_nats::{FlushClient, PublishClient, RequestClient};

use crate::codec::{Encoded, decode, encode};
use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::direction::Direction;
use crate::error::CodecError;
use crate::id::RequestId;
use crate::message::Message;

/// Overlay every header from an encoded message onto a base header map.
pub fn merge_headers(mut base: HeaderMap, overlay: HeaderMap) -> HeaderMap {
    for (name, values) in overlay.iter() {
        let Some((first, rest)) = values.split_first() else {
            continue;
        };
        base.insert(name.clone(), first.clone());
        for value in rest {
            base.append(name.clone(), value.clone());
        }
    }
    base
}

/// Overlay `Jsonrpc-*` headers from an encoded message onto a base header map.
pub fn merge_jsonrpc_headers(mut base: HeaderMap, overlay: HeaderMap) -> HeaderMap {
    if let Some(id) = overlay.get(HEADER_ID) {
        base.insert(HEADER_ID, id.as_str());
    }
    if let Some(code) = overlay.get(HEADER_ERROR_CODE) {
        base.insert(HEADER_ERROR_CODE, code.as_str());
    }
    base
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("NATS request timed out on subject {subject}")]
    Timeout { subject: String },
    #[error("NATS request failed on subject {subject}: {error}")]
    Request { subject: String, error: String },
    #[error("NATS publish failed on subject {subject}: {error}")]
    Publish { subject: String, error: String },
    #[error("NATS publish timed out on subject {subject}")]
    PublishTimeout { subject: String },
    #[error("NATS flush failed: {error}")]
    Flush { error: String },
    #[error("unexpected JSON-RPC response variant")]
    UnexpectedResponse,
}

/// Core NATS request/reply at the byte level: send `headers`/`body` with a
/// timeout and return the raw response headers and body. Callers decode the
/// response into their own representation, either the generic [`Message`] (see
/// [`jsonrpc_request_with_timeout`]) or a domain-typed message.
pub async fn jsonrpc_request_raw<N>(
    client: &N,
    subject: &str,
    headers: HeaderMap,
    body: Bytes,
    timeout: Duration,
) -> Result<(HeaderMap, Bytes), TransportError>
where
    N: RequestClient,
{
    let response = tokio::time::timeout(timeout, client.request_with_headers(subject.to_string(), headers, body))
        .await
        .map_err(|_| TransportError::Timeout {
            subject: subject.to_string(),
        })?
        .map_err(|error| TransportError::Request {
            subject: subject.to_string(),
            error: error.to_string(),
        })?;

    Ok((response.headers.unwrap_or_default(), response.payload))
}

/// Core NATS request/reply for a JSON-RPC call in canonical encoding (ADR#0056),
/// returning the decoded generic [`Message`].
///
/// `method` is the protocol method string written into the body (not the subject
/// terminal projection).
pub async fn jsonrpc_request_with_timeout<N>(
    client: &N,
    subject: &str,
    method: &str,
    request_id: RequestId,
    params: &impl Serialize,
    base_headers: HeaderMap,
    timeout: Duration,
) -> Result<Message, TransportError>
where
    N: RequestClient,
{
    let params = serde_json::to_value(params).map_err(CodecError::Serialize)?;
    let encoded = encode(&Message::Request {
        id: request_id,
        method: method.to_string(),
        params,
    })?;
    let headers = merge_jsonrpc_headers(base_headers, encoded.headers);

    let (response_headers, response_body) =
        jsonrpc_request_raw(client, subject, headers, encoded.body, timeout).await?;
    match decode(Direction::Response, None, &response_headers, &response_body)? {
        message @ (Message::Success { .. } | Message::Error { .. }) => Ok(message),
        _ => Err(TransportError::UnexpectedResponse),
    }
}

/// Publish an encoded JSON-RPC notification or response.
pub async fn jsonrpc_publish<N>(
    client: &N,
    subject: &str,
    encoded: Encoded,
    base_headers: HeaderMap,
) -> Result<(), TransportError>
where
    N: PublishClient + FlushClient,
{
    let headers = merge_headers(base_headers, encoded.headers);
    client
        .publish_with_headers(subject.to_string(), headers, encoded.body)
        .await
        .map_err(|error| TransportError::Publish {
            subject: subject.to_string(),
            error: error.to_string(),
        })?;
    client.flush().await.map_err(|error| TransportError::Flush {
        error: error.to_string(),
    })
}

/// Publish an encoded JSON-RPC message with a deadline covering both the
/// publish and the flush.
pub async fn jsonrpc_publish_with_timeout<N>(
    client: &N,
    subject: &str,
    encoded: Encoded,
    base_headers: HeaderMap,
    timeout: Duration,
) -> Result<(), TransportError>
where
    N: PublishClient + FlushClient,
{
    tokio::time::timeout(timeout, jsonrpc_publish(client, subject, encoded, base_headers))
        .await
        .map_err(|_| TransportError::PublishTimeout {
            subject: subject.to_string(),
        })?
}
