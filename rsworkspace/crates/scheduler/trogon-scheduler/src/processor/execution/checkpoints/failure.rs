//! Durable processing-failure records.
//!
//! A record that cannot be mapped to a schedule checkpoint record (a malformed or
//! undecodable payload) must be acknowledged only after the failure itself is
//! durably visible. These records live in the same KV bucket, keyed by event
//! stream name and `stream_position`, so the poison record is observable for
//! operations before the NATS message is terminated.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use trogon_decider_runtime::StreamPosition;

/// A durable record of a payload the processor could not reconcile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessingFailureRecord {
    /// Event stream the poison record was delivered from.
    pub stream_name: String,
    /// Position of the poison record inside that stream.
    pub stream_position: u64,
    /// Event id of the poison record when available, for correlation.
    pub event_id: Option<String>,
    /// Human-readable reason the record could not be processed.
    pub reason: String,
    /// When the failure was recorded.
    pub recorded_at: DateTime<Utc>,
}

impl ProcessingFailureRecord {
    /// Builds a failure record for the given stream coordinates.
    pub fn new(
        stream_name: impl Into<String>,
        stream_position: StreamPosition,
        event_id: Option<String>,
        reason: impl Into<String>,
        recorded_at: DateTime<Utc>,
    ) -> Self {
        Self {
            stream_name: stream_name.into(),
            stream_position: stream_position.as_u64(),
            event_id,
            reason: reason.into(),
            recorded_at,
        }
    }

    /// Deterministic KV key under which the failure record is stored.
    pub fn key(&self) -> String {
        failure_key(&self.stream_name, self.stream_position)
    }
}

/// Builds the deterministic KV key for a processing-failure record.
pub fn failure_key(stream_name: &str, stream_position: u64) -> String {
    format!("failure.v1.{stream_name}.{stream_position}")
}

/// Encodes a failure record for the KV bucket.
pub fn encode_failure_record(record: &ProcessingFailureRecord) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(record)
}

/// Decodes a failure record from the KV bucket.
#[cfg(test)]
pub fn decode_failure_record(bytes: &[u8]) -> Result<ProcessingFailureRecord, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests;
