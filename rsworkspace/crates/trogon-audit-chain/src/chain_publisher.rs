//! Publisher wrapper that stamps chain headers on every JetStream publish.

use std::future::IntoFuture;
use std::sync::Mutex;

use async_nats::HeaderMap;
use async_nats::jetstream::publish::PublishAck;
use async_nats::subject::ToSubject;
use bytes::Bytes;

use crate::canonical_event_bytes::CanonicalEventBytes;
use crate::chain_hash::ChainHash;
use crate::chain_headers::write_chain_headers;
use crate::extend::extend;
use trogon_nats::jetstream::JetStreamPublisher;

/// Publishes canonical events to JetStream, stamping each message with
/// [`crate::chain_headers`] that link it to the previous published message.
///
/// `ChainPublisher` owns the current chain tip in memory and advances it only
/// after a successful, non-duplicate publish acknowledgement. It is meant for
/// a single logical chain (for example, one subject or one stream): sharing
/// one instance across independent chains would interleave unrelated events
/// into the same hash chain.
///
/// This publisher does not itself provide exclusivity across multiple
/// writers or process restarts. See the crate README's threat model for what
/// that residual risk means and how to close it (typically: a single-writer
/// subject plus `verify_chain` over the durable stream as the source of
/// truth, not this in-memory tip).
pub struct ChainPublisher<P> {
    publisher: P,
    tip: Mutex<ChainHash>,
}

impl<P> ChainPublisher<P>
where
    P: JetStreamPublisher,
{
    /// Creates a publisher whose chain starts from [`ChainHash::genesis`].
    pub fn new(publisher: P) -> Self {
        Self {
            publisher,
            tip: Mutex::new(ChainHash::genesis()),
        }
    }

    /// Creates a publisher that resumes an existing chain from `tip`.
    ///
    /// Callers resuming a chain after a restart must supply the `hash` of
    /// the last successfully verified entry (for example, from
    /// [`crate::verify_chain`] run over the durable stream), not a value
    /// read from this process's own memory.
    pub fn resume(publisher: P, tip: ChainHash) -> Self {
        Self {
            publisher,
            tip: Mutex::new(tip),
        }
    }

    /// Returns the current chain tip.
    pub fn current_tip(&self) -> ChainHash {
        let guard = self.tip.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clone()
    }

    /// Publishes `event_bytes`, stamping chain headers computed against the
    /// current tip, and advances the tip on a successful, non-duplicate ack.
    ///
    /// `headers` may carry additional application headers; the chain headers
    /// are added on top of whatever the caller supplies. Passing headers that
    /// already set the chain header names is a caller error the underlying
    /// `HeaderMap` will silently overwrite as this function's writes happen
    /// last.
    pub async fn publish<S>(
        &self,
        subject: S,
        mut headers: HeaderMap,
        event_bytes: CanonicalEventBytes,
    ) -> Result<PublishAck, ChainPublishError<P::PublishError>>
    where
        S: ToSubject + Send,
    {
        let previous_hash = self.current_tip();
        let hash = extend(&previous_hash, &event_bytes);
        write_chain_headers(&mut headers, &previous_hash, &hash);

        let payload = Bytes::copy_from_slice(event_bytes.as_bytes());
        let ack_future = self
            .publisher
            .publish_with_headers(subject, headers, payload)
            .await
            .map_err(ChainPublishError::Publish)?;

        let ack = ack_future.into_future().await.map_err(ChainPublishError::Ack)?;

        if !ack.duplicate {
            self.advance_tip(hash);
        }

        Ok(ack)
    }

    fn advance_tip(&self, hash: ChainHash) {
        let mut guard = self.tip.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = hash;
    }
}

/// Error returned when [`ChainPublisher::publish`] fails.
#[derive(Debug, thiserror::Error)]
pub enum ChainPublishError<E> {
    /// The underlying JetStream publish call failed.
    #[error("failed to publish chained event: {0}")]
    Publish(#[source] E),
    /// The underlying JetStream publish acknowledgement failed.
    #[error("failed to acknowledge chained event publish: {0}")]
    Ack(#[source] E),
}

#[cfg(test)]
mod tests;
