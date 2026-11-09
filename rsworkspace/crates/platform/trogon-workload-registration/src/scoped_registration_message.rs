use async_nats::jetstream::message::StreamMessage;
use bytes::Bytes;

use crate::{MAX_SNAPSHOT_BYTES, RegistrationError};

#[derive(Debug)]
pub(crate) struct ScopedRegistrationMessage(Bytes);

impl ScopedRegistrationMessage {
    pub(crate) fn new(subject: &str, sequence: u64, message: StreamMessage) -> Result<Self, RegistrationError> {
        if message.subject.as_str() != subject || message.sequence != sequence {
            return Err(RegistrationError::HeadChanged);
        }
        if message.payload.len() > MAX_SNAPSHOT_BYTES {
            return Err(RegistrationError::Bounds);
        }
        Ok(Self(message.payload))
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn into_payload(self) -> Bytes {
        self.0
    }
}

#[cfg(test)]
mod tests;
