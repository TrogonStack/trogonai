use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use trogon_sandbox_protocol::{NatsTransferError, nats_transfer};

use crate::constants::CHUNKED_SUBJECT_PREFIXES;
use crate::{Bus, BusError, BusMessage};

/// The [`Bus`] backed by a NATS connection.
///
/// Requests on the file transfer subjects go through the chunked transfer protocol so payloads larger than the
/// server's maximum payload still cross the wire.
pub struct NatsBus {
    client: async_nats::Client,
}

impl NatsBus {
    pub fn new(client: async_nats::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Bus for NatsBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        timeout: Duration,
        headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        if CHUNKED_SUBJECT_PREFIXES
            .iter()
            .any(|prefix| subject.starts_with(prefix))
        {
            return match nats_transfer::request(&self.client, subject, payload, headers, timeout).await {
                Ok(payload) => Ok(Some(payload)),
                Err(NatsTransferError::RequestTimedOut) => Ok(None),
                // The inner request carries the same deadline as the transfer, so either may expire first.
                Err(NatsTransferError::Request(error))
                    if error.kind() == async_nats::client::RequestErrorKind::TimedOut =>
                {
                    Ok(None)
                }
                Err(error) => Err(BusError::Transfer(error)),
            };
        }
        // Build the request through `Request` so the caller's deadline governs a slow reply, such as a provider's
        // cold golden-snapshot build, instead of the transport's own 10s default.
        let mut request = async_nats::Request::new().payload(payload).timeout(Some(timeout));
        if let Some(headers) = headers {
            request = request.headers(headers);
        }
        match self.client.send_request(subject.to_string(), request).await {
            Ok(message) => Ok(Some(message.payload)),
            Err(error) if error.kind() == async_nats::client::RequestErrorKind::TimedOut => Ok(None),
            Err(error) => Err(BusError::Request(error)),
        }
    }

    async fn subscribe(&self, subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        let subscriber = self.client.subscribe(subject.to_string()).await?;
        Ok(Box::pin(subscriber.map(|message| BusMessage {
            subject: message.subject.to_string(),
            payload: message.payload,
        })))
    }
}
