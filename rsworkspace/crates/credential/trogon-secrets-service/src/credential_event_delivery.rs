use async_nats::jetstream::Message;
use async_nats::jetstream::message::StreamMessage;
use trogon_nats::jetstream::{JsAck, JsAckWith};

use super::DeliveryInfoError;

/// A credential event as the stream it was read from stored it.
#[derive(Clone, Debug)]
pub struct StoredCredentialEvent {
    /// The name of the stream the event was read from, which names the event
    /// in a notice's `Nats-Msg-Id` because the credential stream's name is the
    /// operator's to configure.
    pub stream: String,
    /// The message the stream stored.
    pub message: StreamMessage,
}

/// One credential stream message as a consumer delivered it: what the
/// invalidation publisher reads, and what it settles once it is done.
///
/// It is a port of its own because a delivery's stream sequence and append
/// time live in its acknowledgement subject, which only a real delivery can
/// carry, so the publisher's settling can be tested without one.
pub trait CredentialEventDelivery: JsAck + JsAckWith {
    /// The message as the credential stream stored it: its subject, headers,
    /// and payload, with the stream sequence and the append time the delivery
    /// reports, and the name of the stream it was read from.
    ///
    /// # Errors
    ///
    /// Returns [`DeliveryInfoError`] when the delivery does not report where
    /// the stream stored the message.
    fn stored(&self) -> Result<StoredCredentialEvent, DeliveryInfoError>;
}

impl CredentialEventDelivery for Message {
    fn stored(&self) -> Result<StoredCredentialEvent, DeliveryInfoError> {
        let info = self.info().map_err(DeliveryInfoError)?;
        Ok(StoredCredentialEvent {
            stream: info.stream.to_owned(),
            message: StreamMessage {
                subject: self.message.subject.clone(),
                sequence: info.stream_sequence,
                headers: self.message.headers.clone().unwrap_or_default(),
                payload: self.message.payload.clone(),
                time: info.published,
            },
        })
    }
}
