//! Bounded request/reply payloads for file operations across NATS hops.
//!
//! A payload that fits one message travels inline. A larger one is replaced by
//! a small descriptor naming an inbox; the receiver pulls the payload from that
//! inbox chunk by chunk, so no single message exceeds the server's limit.

use std::time::Duration;

use async_nats::{Client, HeaderMap, Message};
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

pub use crate::constants::MAX_TRANSFER_BYTES;
use crate::constants::{MAX_CHUNK_BYTES, METADATA_RESERVE, TRANSFER_HEADER, TRANSFER_TIMEOUT, TRANSFER_VERSION};
use crate::dispatch_global::check_reassembled;
use crate::nats_transfer_error::NatsTransferError;

#[derive(Serialize, Deserialize)]
struct Transfer {
    inbox: String,
    length: usize,
    chunk_size: usize,
}

impl Transfer {
    /// Parses and bounds-checks a descriptor received from a peer.
    fn parse(payload: &[u8]) -> Result<Self, NatsTransferError> {
        let transfer: Self = serde_json::from_slice(payload)?;
        if transfer.length == 0 || transfer.length > MAX_TRANSFER_BYTES {
            return Err(NatsTransferError::InvalidLength);
        }
        if transfer.chunk_size == 0 || transfer.chunk_size > MAX_CHUNK_BYTES {
            return Err(NatsTransferError::InvalidChunkSize);
        }
        if !transfer.inbox.starts_with("_INBOX.") || transfer.inbox.contains(['*', '>', ' ', '\r', '\n']) {
            return Err(NatsTransferError::InvalidInbox);
        }
        Ok(transfer)
    }
}

/// Aborts the chunk sender when the transfer is abandoned.
struct ServerTask(tokio::task::JoinHandle<Result<(), NatsTransferError>>);

impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn prepare(
    client: &Client,
    payload: Bytes,
    mut headers: HeaderMap,
    stop_after_last: bool,
) -> Result<(Bytes, HeaderMap, Option<ServerTask>), NatsTransferError> {
    if payload.len() > MAX_TRANSFER_BYTES {
        return Err(NatsTransferError::TooLarge {
            max: MAX_TRANSFER_BYTES,
        });
    }
    let chunk_size = client
        .server_info()
        .max_payload
        .checked_sub(METADATA_RESERVE)
        .ok_or(NatsTransferError::NoRoomForMetadata)?
        .min(MAX_CHUNK_BYTES);
    if payload.len() <= chunk_size {
        return Ok((payload, headers, None));
    }
    let inbox = client.new_inbox();
    let mut chunks = client.subscribe(inbox.clone()).await?;
    let descriptor = serde_json::to_vec(&Transfer {
        inbox,
        length: payload.len(),
        chunk_size,
    })?;
    let sender = client.clone();
    let task = ServerTask(tokio::spawn(async move {
        tokio::time::timeout(TRANSFER_TIMEOUT, async move {
            while let Some(message) = chunks.next().await {
                let Some(reply) = message.reply else {
                    continue;
                };
                let Ok(offset) = std::str::from_utf8(&message.payload).unwrap_or("").parse::<usize>() else {
                    continue;
                };
                if offset >= payload.len() || offset % chunk_size != 0 {
                    continue;
                }
                let end = offset.saturating_add(chunk_size).min(payload.len());
                sender.publish(reply, payload.slice(offset..end)).await?;
                if stop_after_last && end == payload.len() {
                    return Ok(());
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| NatsTransferError::SenderTimedOut)?
    }));
    headers.insert(TRANSFER_HEADER, TRANSFER_VERSION);
    Ok((descriptor.into(), headers, Some(task)))
}

/// Resolves either an ordinary inline payload or a bounded transfer descriptor.
pub async fn receive(client: &Client, message: &Message) -> Result<Bytes, NatsTransferError> {
    let Some((headers, version)) = message
        .headers
        .as_ref()
        .and_then(|headers| headers.get(TRANSFER_HEADER).map(|version| (headers, version)))
    else {
        return Ok(message.payload.clone());
    };
    if version.as_str() != TRANSFER_VERSION {
        return Err(NatsTransferError::UnknownVersion);
    }
    let transfer = Transfer::parse(&message.payload)?;
    tokio::time::timeout(TRANSFER_TIMEOUT, async {
        let mut data = BytesMut::new();
        while data.len() < transfer.length {
            let offset = data.len();
            let reply = client
                .send_request(
                    transfer.inbox.clone(),
                    async_nats::Request::new()
                        .payload(offset.to_string().into())
                        .timeout(Some(TRANSFER_TIMEOUT)),
                )
                .await?;
            let expected = transfer.chunk_size.min(transfer.length - offset);
            if reply.payload.len() != expected {
                return Err(NatsTransferError::InvalidChunkLength);
            }
            data.extend_from_slice(&reply.payload);
        }
        let data = data.freeze();
        check_reassembled(headers, &data)?;
        Ok(data)
    })
    .await
    .map_err(|_| NatsTransferError::ReceiverTimedOut)?
}

/// Sends a request with a deadline covering both its payload and reply transfers.
pub async fn request(
    client: &Client,
    subject: &str,
    payload: Bytes,
    headers: Option<HeaderMap>,
    timeout: Duration,
) -> Result<Bytes, NatsTransferError> {
    tokio::time::timeout(timeout, async {
        let (payload, headers, _server) = prepare(client, payload, headers.unwrap_or_default(), false).await?;
        let message = client
            .send_request(
                subject.to_owned(),
                async_nats::Request::new()
                    .payload(payload)
                    .headers(headers)
                    .timeout(Some(timeout)),
            )
            .await?;
        receive(client, &message).await
    })
    .await
    .map_err(|_| NatsTransferError::RequestTimedOut)?
}

/// Publishes a reply, serving large payloads until read or the transfer expires.
pub async fn respond(client: &Client, reply: &str, payload: Bytes) -> Result<(), NatsTransferError> {
    let (payload, headers, server) = prepare(client, payload, HeaderMap::new(), true).await?;
    client.publish_with_headers(reply.to_owned(), headers, payload).await?;
    if let Some(mut server) = server {
        (&mut server.0).await??;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
