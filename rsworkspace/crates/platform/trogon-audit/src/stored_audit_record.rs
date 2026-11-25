use async_nats::header::NATS_MESSAGE_ID;
use async_nats::{HeaderMap, HeaderName};
use bytes::Bytes;

use crate::constants::{HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER};

/// One message read back from the `AUDIT` stream by a reader that may read it:
/// the payload exactly as stored, the stream sequence the server gave it, and the
/// subject and provenance headers when the reader has them.
///
/// The stream sequence must be the server's, never one the reader numbered, so a
/// verifier can tell that records arrived in the order they were stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAuditRecord {
    stream_sequence: u64,
    payload: Bytes,
    subject: Option<String>,
    message_id: Option<String>,
    writer: Option<String>,
    sequence: Option<String>,
    chain: Option<String>,
}

impl StoredAuditRecord {
    pub fn new(stream_sequence: u64, payload: impl Into<Bytes>) -> Self {
        Self {
            stream_sequence,
            payload: payload.into(),
            subject: None,
            message_id: None,
            writer: None,
            sequence: None,
            chain: None,
        }
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Takes `Nats-Msg-Id`, `Trogon-Audit-Writer`, `Trogon-Audit-Seq` and
    /// `Trogon-Audit-Chain`; a header the map does not hold stays unknown.
    pub fn with_headers(mut self, headers: &HeaderMap) -> Self {
        let value = |name: HeaderName| headers.get(name).map(|value| value.as_str().to_owned());
        self.message_id = value(NATS_MESSAGE_ID);
        self.writer = value(HeaderName::from_static(HEADER_WRITER));
        self.sequence = value(HeaderName::from_static(HEADER_SEQUENCE));
        self.chain = value(HeaderName::from_static(HEADER_CHAIN));
        self
    }

    pub fn stream_sequence(&self) -> u64 {
        self.stream_sequence
    }

    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    pub fn writer(&self) -> Option<&str> {
        self.writer.as_deref()
    }

    pub fn sequence(&self) -> Option<&str> {
        self.sequence.as_deref()
    }

    pub fn chain(&self) -> Option<&str> {
        self.chain.as_deref()
    }

    /// True when the reader supplied the subject and every provenance header, so
    /// the record's transport metadata was checked and not only its payload.
    pub fn has_metadata(&self) -> bool {
        self.subject.is_some()
            && self.message_id.is_some()
            && self.writer.is_some()
            && self.sequence.is_some()
            && self.chain.is_some()
    }
}

#[cfg(test)]
mod tests;
