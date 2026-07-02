//! Replays a JetStream stream and verifies its audit hash chain.

use trogon_nats::jetstream::{JetStreamGetRawMessage, JetStreamGetStreamInfo};

use crate::canonical_event_bytes::{CanonicalEventBytes, CanonicalizeError};
use crate::chain_break::ChainBreak;
use crate::chain_entry::ChainEntry;
use crate::chain_headers::{ChainHeaderError, read_chain_headers};
use crate::chain_sequence::ChainSequence;
use crate::verify_chain::verify_chain;

/// Reads every message from `stream` in ascending stream-sequence order and
/// verifies that the chain headers on those messages form an unbroken hash
/// chain.
///
/// This treats the JetStream stream sequence as the chain sequence directly:
/// message 1 is chain position 1, message 2 is chain position 2, and so on.
/// That is correct for a stream dedicated to one audit chain (the design this
/// crate assumes; see the README). A stream that interleaves audit events
/// with unrelated messages is out of scope: filter to the audit subject
/// before calling this function, or give the audit chain its own stream.
///
/// Deleted messages inside the sequence range surface as a JetStream
/// "no message found" condition, which this function reports as
/// [`ReplayVerifyError::MissingMessage`] rather than silently skipping the
/// gap; skipping would let a stream-delete-capable actor erase an entry
/// undetected.
pub async fn verify_stream_chain<S>(stream: &S) -> Result<(), ReplayVerifyError>
where
    S: JetStreamGetStreamInfo + JetStreamGetRawMessage,
{
    let info = stream.get_info().await.map_err(ReplayVerifyError::StreamInfo)?;
    let last_sequence = info.state.last_sequence;

    let mut entries = Vec::new();
    for raw_sequence in 1..=last_sequence {
        let message =
            stream
                .get_raw_message(raw_sequence)
                .await
                .map_err(|source| ReplayVerifyError::MissingMessage {
                    sequence: raw_sequence,
                    source,
                })?;

        let sequence = ChainSequence::try_new(raw_sequence).map_err(|_| ReplayVerifyError::ZeroSequence)?;
        let (previous_hash, hash) =
            read_chain_headers(&message.headers).map_err(|source| ReplayVerifyError::Header { sequence, source })?;
        let event_bytes = CanonicalEventBytes::from_json_slice(&message.payload)
            .map_err(|source| ReplayVerifyError::Canonicalize { sequence, source })?;

        entries.push(ChainEntry::new(sequence, previous_hash, event_bytes, hash));
    }

    verify_chain(entries).map_err(ReplayVerifyError::Break)
}

/// Error returned by [`verify_stream_chain`].
#[derive(Debug, thiserror::Error)]
pub enum ReplayVerifyError {
    /// Fetching stream info failed.
    #[error("failed to query stream info: {0}")]
    StreamInfo(#[source] async_nats::jetstream::stream::InfoError),
    /// A message inside the stream's sequence range could not be read.
    ///
    /// This includes messages removed by a subject purge or a direct
    /// message delete; see the crate README's threat model for why that is
    /// treated as a verification failure rather than a skip.
    #[error("stream message at sequence {sequence} could not be read: {source}")]
    MissingMessage {
        /// The stream sequence that could not be read.
        sequence: u64,
        /// The underlying JetStream error.
        #[source]
        source: async_nats::jetstream::stream::RawMessageError,
    },
    /// A raw JetStream sequence of zero was encountered, which should be
    /// impossible because JetStream sequences start at 1.
    #[error("encountered a zero stream sequence, which JetStream should never assign")]
    ZeroSequence,
    /// A message was missing or had malformed chain headers.
    #[error("stream message at sequence {sequence} has invalid chain headers: {source}")]
    Header {
        /// The chain sequence of the offending message.
        sequence: ChainSequence,
        /// The underlying header error.
        #[source]
        source: ChainHeaderError,
    },
    /// A message's payload could not be canonicalized.
    #[error("stream message at sequence {sequence} could not be canonicalized: {source}")]
    Canonicalize {
        /// The chain sequence of the offending message.
        sequence: ChainSequence,
        /// The underlying canonicalization error.
        #[source]
        source: CanonicalizeError,
    },
    /// The chain itself failed to verify.
    #[error("audit chain verification failed: {0}")]
    Break(#[source] ChainBreak),
}

#[cfg(test)]
mod tests;
