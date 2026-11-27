use super::*;

#[async_trait]
pub trait CommandLogStore: Send + Sync {
    /// Chunks of `execution_id` with `log_seq > after_seq`, at most
    /// `max_bytes` of data (but always at least one chunk when any is left).
    async fn read(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        after_seq: u64,
        max_bytes: u64,
    ) -> Result<LogPage, CommandLogError>;

    /// Returns once a chunk with `log_seq > after_seq` may exist for
    /// `execution_id`, or after `wait`, whichever is first. A caller that
    /// found nothing to read calls this and then reads again, so a live
    /// follower costs one parked request instead of a polling loop. Returning
    /// early or late is harmless: the read that follows decides.
    async fn wait_for_chunks(
        &self,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        after_seq: u64,
        wait: Duration,
    ) -> Result<(), CommandLogError>;

    /// Drops every retained chunk of every execution on `lease_id`. Called
    /// when a lease created with `retention.monitoring` off ends, so its
    /// command history does not outlive it.
    async fn purge_lease(&self, lease_id: &LeaseId) -> Result<(), CommandLogError>;
}
