use super::jet_stream_command_log_store::{PageBuilder, chunk_subject};
use super::*;

/// Command logs held in memory, applying the same retention rules as the
/// JetStream stream. Backs tests and any deployment without JetStream.
#[cfg_attr(not(test), allow(dead_code))]
pub struct InMemoryCommandLogStore {
    retention: CommandLogRetention,
    inner: Mutex<InMemoryLogs>,
    appended: tokio::sync::Notify,
}

#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
struct InMemoryLogs {
    next_seq: u64,
    bytes: u64,
    /// Every retained chunk in arrival order: `(log_seq, subject, chunk)`.
    order: VecDeque<(u64, String, CommandOutputChunk)>,
    per_subject: BTreeMap<String, usize>,
    /// Exit marker subjects seen.
    exits: BTreeSet<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl InMemoryCommandLogStore {
    pub fn new(retention: CommandLogRetention) -> Self {
        Self {
            retention,
            inner: Mutex::new(InMemoryLogs::default()),
            appended: tokio::sync::Notify::new(),
        }
    }

    /// Stores a chunk as the stream would, then applies retention.
    pub fn append(&self, lease_id: &LeaseId, chunk: CommandOutputChunk) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.next_seq += 1;
        let log_seq = inner.next_seq;
        let subject = chunk_subject(lease_id, &chunk.execution_id);
        inner.bytes += chunk.data.len() as u64;
        *inner.per_subject.entry(subject.clone()).or_default() += 1;
        inner.order.push_back((log_seq, subject.clone(), chunk));

        let per_execution_limit = usize::try_from(self.retention.max_chunks_per_execution)
            .ok()
            .filter(|n| *n > 0);
        if let Some(limit) = per_execution_limit
            && inner.per_subject[&subject] > limit
            && let Some(at) = inner.order.iter().position(|(_, s, _)| *s == subject)
        {
            inner.drop_at(at);
        }
        let max_bytes = u64::try_from(self.retention.max_bytes).ok().filter(|n| *n > 0);
        while let Some(max) = max_bytes
            && inner.bytes > max
            && inner.order.len() > 1
        {
            inner.drop_at(0);
        }
        drop(inner);
        self.appended.notify_waiters();
    }

    /// Stores an exit marker as the stream would.
    pub fn mark_exited(&self, lease_id: &LeaseId, marker: &CommandExitMarker) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .exits
            .insert(command_exit_subject(lease_id, &marker.execution_id));
        self.appended.notify_waiters();
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl InMemoryLogs {
    fn drop_at(&mut self, at: usize) {
        if let Some((_, subject, chunk)) = self.order.remove(at) {
            self.bytes -= chunk.data.len() as u64;
            if let Some(n) = self.per_subject.get_mut(&subject) {
                *n -= 1;
                if *n == 0 {
                    self.per_subject.remove(&subject);
                }
            }
        }
    }
}

#[async_trait]
impl CommandLogStore for InMemoryCommandLogStore {
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
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut builder = PageBuilder::new(after_seq, max_bytes);
        for (log_seq, _, chunk) in inner
            .order
            .iter()
            .filter(|(seq, s, _)| *seq > after_seq && *s == subject)
        {
            if !builder.admits(chunk.data.len()) {
                builder.page.has_more = true;
                break;
            }
            builder.push(*log_seq, chunk.clone());
        }
        let mut page = builder.page;
        page.truncated = inner
            .order
            .iter()
            .find(|(_, s, _)| *s == subject)
            .is_some_and(|(_, _, c)| c.seq != 0);
        page.exited = inner.exits.contains(&command_exit_subject(lease_id, execution_id));
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
        let subject = chunk_subject(lease_id, execution_id);
        let notified = self.appended.notified();
        tokio::pin!(notified);
        // Register before looking so an append in between still wakes us.
        notified.as_mut().enable();
        let has_chunk = {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.order.iter().any(|(seq, s, _)| *seq > after_seq && *s == subject)
                || inner.exits.contains(&command_exit_subject(lease_id, execution_id))
        };
        if !has_chunk {
            let _ = tokio::time::timeout(wait, notified).await;
        }
        Ok(())
    }

    async fn purge_lease(&self, lease_id: &LeaseId) -> Result<(), CommandLogError> {
        let prefix = format!("sandbox.evidence.{}.", lease_id.as_str());
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        while let Some(at) = inner.order.iter().position(|(_, s, _)| s.starts_with(&prefix)) {
            inner.drop_at(at);
        }
        inner.exits.retain(|s| !s.starts_with(&prefix));
        Ok(())
    }
}
