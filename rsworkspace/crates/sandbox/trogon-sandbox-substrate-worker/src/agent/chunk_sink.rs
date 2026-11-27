use async_trait::async_trait;
use trogon_sandbox_contract::CommandOutputChunk;

/// Where live output chunks go while a command is still running.
///
/// The subject depends on the execution id, which the *agent* mints, so the sink derives it
/// per chunk instead of being handed a fixed subject at construction.
#[async_trait]
pub trait ChunkSink: Send + Sync {
    async fn publish(&self, chunk: CommandOutputChunk);

    /// Learns the execution's id before any output, so a command that prints nothing can
    /// still be named. Agents that predate the begin frame never call this.
    async fn begin(&self, _execution_id: String) {}
}
