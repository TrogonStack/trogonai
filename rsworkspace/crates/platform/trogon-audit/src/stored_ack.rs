use async_nats::jetstream::publish::PublishAck;

/// The stream's acknowledgement that it stored a record: which stream, at what
/// stream sequence, and whether the record was already stored under its message
/// id inside the duplicate window. A duplicate's stream sequence is the first
/// copy's, so a retry after a lost ack still learns where the record sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAck {
    stream: String,
    stream_sequence: u64,
    duplicate: bool,
}

impl StoredAck {
    pub fn new(stream: impl Into<String>, stream_sequence: u64, duplicate: bool) -> Self {
        Self {
            stream: stream.into(),
            stream_sequence,
            duplicate,
        }
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn stream_sequence(&self) -> u64 {
        self.stream_sequence
    }

    pub fn duplicate(&self) -> bool {
        self.duplicate
    }
}

impl From<PublishAck> for StoredAck {
    fn from(ack: PublishAck) -> Self {
        Self::new(ack.stream, ack.sequence, ack.duplicate)
    }
}

#[cfg(test)]
mod tests;
