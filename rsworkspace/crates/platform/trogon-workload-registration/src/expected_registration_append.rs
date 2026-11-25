use std::num::NonZeroU64;

use async_nats::jetstream::publish::PublishAck;

use crate::RegistrationError;

#[derive(Debug)]
pub(crate) struct ExpectedRegistrationAppend {
    stream: String,
    sequence: NonZeroU64,
}

impl ExpectedRegistrationAppend {
    pub(crate) fn new(stream: &str, previous: u64) -> Result<Self, RegistrationError> {
        let sequence = previous
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or(RegistrationError::Revision)?;
        Ok(Self {
            stream: stream.to_owned(),
            sequence,
        })
    }

    pub(crate) fn acknowledge(self, ack: &PublishAck) -> Result<NonZeroU64, RegistrationError> {
        if ack.duplicate || ack.stream != self.stream || ack.sequence != self.sequence.get() {
            return Err(RegistrationError::HeadChanged);
        }
        Ok(self.sequence)
    }
}

#[cfg(test)]
mod tests;
