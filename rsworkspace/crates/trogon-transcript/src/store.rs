use crate::{
    entry::TranscriptEntry,
    error::TranscriptError,
    subject::{entity_subject_filter, transcript_subject},
};
use async_nats::jetstream::{
    self,
    consumer::{AckPolicy, DeliverPolicy, pull},
    stream,
};
use futures_util::StreamExt as _;
use tracing::instrument;

pub const STREAM_NAME: &str = "TRANSCRIPTS";

/// Upper bound on entries returned by a single query.
///
/// A full entity transcript is unlikely to exceed this in practice. A proper
/// streaming implementation using JetStream sequence cursors can replace this
/// once the need arises.
const MAX_QUERY_ENTRIES: usize = 10_000;

/// Manages the `TRANSCRIPTS` JetStream stream and provides read access to
/// stored entries.
///
/// `TranscriptStore` uses the real `jetstream::Context` directly — it is tested
/// via integration tests, not unit tests. The write path (used by actors at
/// runtime) goes through `Session<NatsTranscriptPublisher>` instead.
pub struct TranscriptStore {
    js: jetstream::Context,
}

impl TranscriptStore {
    pub fn new(js: jetstream::Context) -> Self {
        Self { js }
    }

    /// Ensure the `TRANSCRIPTS` stream exists with the correct configuration.
    ///
    /// Safe to call multiple times — `get_or_create_stream` is idempotent.
    #[instrument(skip(self), err)]
    pub async fn provision(&self) -> Result<(), TranscriptError> {
        self.js
            .get_or_create_stream(stream::Config {
                name: STREAM_NAME.to_string(),
                subjects: vec!["transcripts.>".to_string()],
                storage: stream::StorageType::File,
                retention: stream::RetentionPolicy::Limits,
                ..Default::default()
            })
            .await
            .map_err(|e| TranscriptError::Provision(e.to_string()))?;
        Ok(())
    }

    /// Return all transcript entries for a given entity, across all sessions,
    /// in the order they were appended.
    ///
    /// The result is bounded by [`MAX_QUERY_ENTRIES`]. For entities with very
    /// long histories, use `replay` to read one session at a time.
    pub async fn query(
        &self,
        actor_type: &str,
        actor_key: &str,
    ) -> Result<Vec<TranscriptEntry>, TranscriptError> {
        let filter = entity_subject_filter(actor_type, actor_key);
        self.fetch_entries(filter).await
    }

    /// Return all transcript entries for a specific session (single actor
    /// invocation), identified by `session_id`.
    pub async fn replay(
        &self,
        actor_type: &str,
        actor_key: &str,
        session_id: &str,
    ) -> Result<Vec<TranscriptEntry>, TranscriptError> {
        let filter = transcript_subject(actor_type, actor_key, session_id);
        self.fetch_entries(filter).await
    }

    /// Create an ephemeral pull consumer filtered to `filter_subject`, fetch
    /// up to `MAX_QUERY_ENTRIES` messages, deserialize them, and return as a
    /// `Vec`. The ephemeral consumer is cleaned up by NATS after its inactivity
    /// threshold elapses.
    async fn fetch_entries(
        &self,
        filter_subject: String,
    ) -> Result<Vec<TranscriptEntry>, TranscriptError> {
        let stream = self
            .js
            .get_stream(STREAM_NAME)
            .await
            .map_err(|e| TranscriptError::Stream(e.to_string()))?;

        let consumer = stream
            .create_consumer(pull::Config {
                filter_subject,
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::None,
                ..Default::default()
            })
            .await
            .map_err(|e| TranscriptError::Stream(e.to_string()))?;

        let mut messages = consumer
            .fetch()
            .max_messages(MAX_QUERY_ENTRIES)
            .messages()
            .await
            .map_err(|e| TranscriptError::Stream(e.to_string()))?;

        let mut entries = Vec::new();
        while let Some(result) = messages.next().await {
            let msg = result.map_err(|e| TranscriptError::Stream(e.to_string()))?;
            let entry = serde_json::from_slice::<TranscriptEntry>(&msg.payload)
                .map_err(TranscriptError::Deserialization)?;
            entries.push(entry);
        }

        Ok(entries)
    }
}
