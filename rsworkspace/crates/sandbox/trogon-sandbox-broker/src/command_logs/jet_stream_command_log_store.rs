use super::*;

fn lease_chunk_filter(lease_id: &LeaseId) -> String {
    format!("sandbox.evidence.{}.command.*", lease_id.as_str())
}

fn lease_exit_filter(lease_id: &LeaseId) -> String {
    format!("sandbox.evidence.{}.command_exit.*", lease_id.as_str())
}

pub(super) fn chunk_subject(lease_id: &LeaseId, execution_id: &ExecutionId) -> String {
    format!(
        "sandbox.evidence.{}.{}",
        lease_id.as_str(),
        evidence_command_suffix(execution_id)
    )
}

/// Accumulates a page, enforcing the byte budget.
pub(super) struct PageBuilder {
    max_bytes: u64,
    bytes: u64,
    pub(super) page: LogPage,
}

impl PageBuilder {
    pub(super) fn new(after_seq: u64, max_bytes: u64) -> Self {
        Self {
            max_bytes,
            bytes: 0,
            page: LogPage {
                next_seq: after_seq,
                ..LogPage::default()
            },
        }
    }

    /// Whether a chunk of `len` bytes still belongs on this page. The first
    /// chunk always does, so a chunk larger than the budget still makes
    /// progress.
    pub(super) fn admits(&self, len: usize) -> bool {
        self.page.chunks.is_empty()
            || (self.bytes + len as u64 <= self.max_bytes && self.page.chunks.len() < MAX_CHUNKS_PER_PAGE)
    }

    pub(super) fn push(&mut self, log_seq: u64, chunk: CommandOutputChunk) {
        self.bytes += chunk.data.len() as u64;
        self.page.next_seq = log_seq;
        self.page.chunks.push(LogChunk {
            log_seq,
            stream: chunk.stream,
            seq: chunk.seq,
            data: chunk.data,
        });
    }
}

/// Command logs held in a JetStream stream.
pub struct JetStreamCommandLogStore {
    stream: stream::Stream,
    /// Longer-lived copy of the exit markers; `None` when it could not be
    /// created, in which case exit status lives only as long as the chunks.
    exit_stream: Option<stream::Stream>,
    /// How long after a purge the same filters are purged again.
    late_chunk_sweep: Duration,
}

impl JetStreamCommandLogStore {
    /// Creates the stream when absent. Fails when another stream already
    /// captures the same subjects; the caller then runs without command logs.
    pub async fn new(
        jetstream: jetstream::Context,
        retention: CommandLogRetention,
        num_replicas: usize,
    ) -> Result<Self, CommandLogError> {
        let stream = jetstream
            .get_or_create_stream(stream::Config {
                name: COMMAND_LOG_STREAM.to_string(),
                subjects: vec![
                    COMMAND_LOG_SUBJECT_FILTER.to_string(),
                    COMMAND_EXIT_SUBJECT_FILTER.to_string(),
                ],
                retention: stream::RetentionPolicy::Limits,
                discard: stream::DiscardPolicy::Old,
                max_bytes: retention.max_bytes,
                max_age: retention.max_age,
                max_messages_per_subject: retention.max_chunks_per_execution,
                num_replicas,
                storage: stream::StorageType::File,
                allow_direct: true,
                ..Default::default()
            })
            .await
            .map_err(CommandLogError::backend)?;
        let mut stream = stream;
        let mut config = stream.cached_info().config.clone();
        if !config.subjects.iter().any(|s| s == COMMAND_EXIT_SUBJECT_FILTER) {
            // A stream created before exit markers existed: widen it in place.
            // Should that fail, followers still end by polling the command.
            config.subjects.push(COMMAND_EXIT_SUBJECT_FILTER.to_string());
            match jetstream.update_stream(&config).await {
                Ok(_) => {
                    stream = jetstream
                        .get_stream(COMMAND_LOG_STREAM)
                        .await
                        .map_err(CommandLogError::backend)?
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "command log stream does not capture exit markers; log followers fall back to polling"
                ),
            }
        }
        let exit_stream = match jetstream
            .get_or_create_stream(stream::Config {
                name: COMMAND_EXIT_STREAM.to_string(),
                sources: Some(vec![stream::Source {
                    name: COMMAND_LOG_STREAM.to_string(),
                    filter_subject: Some(COMMAND_EXIT_SUBJECT_FILTER.to_string()),
                    ..Default::default()
                }]),
                retention: stream::RetentionPolicy::Limits,
                discard: stream::DiscardPolicy::Old,
                max_age: retention.exit_max_age.max(retention.max_age),
                max_messages_per_subject: 1,
                num_replicas,
                storage: stream::StorageType::File,
                allow_direct: true,
                ..Default::default()
            })
            .await
        {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "command exit stream unavailable; exit status ages out with the output"
                );
                None
            }
        };
        Ok(Self {
            stream,
            exit_stream,
            late_chunk_sweep: LATE_CHUNK_SWEEP,
        })
    }

    #[cfg(test)]
    pub(super) fn with_late_chunk_sweep(mut self, delay: Duration) -> Self {
        self.late_chunk_sweep = delay;
        self
    }
}

fn is_not_found(e: &jetstream::stream::DirectGetError) -> bool {
    matches!(e.kind(), jetstream::stream::DirectGetErrorKind::NotFound)
}

impl JetStreamCommandLogStore {
    /// Whether the long-lived exit record exists, for a command whose marker
    /// already aged out of the log stream.
    async fn exit_record_exists(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
    ) -> Result<bool, CommandLogError> {
        let Some(exit_stream) = &self.exit_stream else {
            return Ok(false);
        };
        match exit_stream
            .direct_get_last_for_subject(command_exit_subject(lease_id, execution_id))
            .await
        {
            Ok(marker) => Ok(serde_json::from_slice::<CommandExitMarker>(&marker.payload).is_ok()),
            Err(e) if is_not_found(&e) => Ok(false),
            Err(e) => Err(CommandLogError::backend(e)),
        }
    }
}

#[async_trait]
impl CommandLogStore for JetStreamCommandLogStore {
    async fn read(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        after_seq: u64,
        max_bytes: u64,
    ) -> Result<LogPage, CommandLogError> {
        if !valid_execution_id(execution_id) {
            return Err(CommandLogError::InvalidExecution);
        }
        let subject = chunk_subject(lease_id, execution_id);

        // Look for the exit marker before reading chunks. The worker writes
        // it after its last chunk, so seeing it first means the page below
        // holds everything; looking afterwards could miss a final chunk that
        // landed in between.
        let exited = match self
            .stream
            .direct_get_last_for_subject(command_exit_subject(lease_id, execution_id))
            .await
        {
            Ok(marker) => serde_json::from_slice::<CommandExitMarker>(&marker.payload).is_ok(),
            Err(e) if is_not_found(&e) => self.exit_record_exists(lease_id, execution_id).await?,
            Err(e) => return Err(CommandLogError::backend(e)),
        };
        let mut builder = PageBuilder::new(after_seq, max_bytes);
        let mut next = after_seq.saturating_add(1);
        loop {
            let message = match self
                .stream
                .direct_get_next_for_subject(subject.clone(), Some(next))
                .await
            {
                Ok(m) => m,
                Err(e) if is_not_found(&e) => break,
                Err(e) => return Err(CommandLogError::backend(e)),
            };
            next = message.sequence.saturating_add(1);
            let Ok(chunk) = serde_json::from_slice::<CommandOutputChunk>(&message.payload) else {
                // Not a chunk this reader understands. Skip it but keep the
                // cursor moving so the page does not stall on it.
                builder.page.next_seq = builder.page.next_seq.max(message.sequence);
                continue;
            };
            if !builder.admits(chunk.data.len()) {
                builder.page.has_more = true;
                break;
            }
            builder.push(message.sequence, chunk);
        }

        let mut page = builder.page;
        page.truncated = match self.stream.direct_get_first_for_subject(subject.clone()).await {
            // Output is dropped oldest first, so the first retained chunk is
            // still the execution's first (`seq` 0) only while nothing before
            // it was dropped.
            Ok(first) => serde_json::from_slice::<CommandOutputChunk>(&first.payload)
                .map(|c| c.seq != 0)
                .unwrap_or(false),
            Err(e) if is_not_found(&e) => false,
            Err(e) => return Err(CommandLogError::backend(e)),
        };
        page.exited = exited;
        Ok(page)
    }

    async fn wait_for_chunks(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        after_seq: u64,
        wait: Duration,
    ) -> Result<(), CommandLogError> {
        if !valid_execution_id(execution_id) {
            return Err(CommandLogError::InvalidExecution);
        }
        // An ephemeral consumer that starts at the cursor: it sees a chunk
        // stored after the caller's read but before this call, so no wakeup
        // is lost, and it holds the request open until the next one lands.
        let consumer = self
            .stream
            .create_consumer(consumer::pull::Config {
                filter_subjects: vec![
                    chunk_subject(lease_id, execution_id),
                    command_exit_subject(lease_id, execution_id),
                ],
                deliver_policy: consumer::DeliverPolicy::ByStartSequence {
                    start_sequence: after_seq.saturating_add(1),
                },
                ack_policy: consumer::AckPolicy::None,
                inactive_threshold: wait + Duration::from_secs(5),
                ..Default::default()
            })
            .await
            .map_err(CommandLogError::backend)?;
        let mut batch = consumer
            .fetch()
            .max_messages(1)
            .expires(wait)
            .messages()
            .await
            .map_err(CommandLogError::backend)?;
        let _ = futures::StreamExt::next(&mut batch).await;
        let _ = self.stream.delete_consumer(&consumer.cached_info().name).await;
        Ok(())
    }

    async fn purge_lease(&self, lease_id: &LeaseId) -> Result<(), CommandLogError> {
        let filters = [lease_chunk_filter(lease_id), lease_exit_filter(lease_id)];
        purge_filters(&self.stream, &filters).await?;
        if let Some(exit_stream) = &self.exit_stream {
            purge_filters(exit_stream, &filters[1..]).await?;
        }
        // The second pass is best effort: the first already ran, and the
        // stream's own age limit expires anything this misses.
        let stream = self.stream.clone();
        let exit_stream = self.exit_stream.clone();
        let delay = self.late_chunk_sweep;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Err(e) = purge_filters(&stream, &filters).await {
                tracing::warn!(error = %e, "late command chunk purge failed");
            }
            if let Some(exit_stream) = exit_stream
                && let Err(e) = purge_filters(&exit_stream, &filters[1..]).await
            {
                tracing::warn!(error = %e, "late command exit purge failed");
            }
        });
        Ok(())
    }
}
