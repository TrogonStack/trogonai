use async_trait::async_trait;
use trogon_sandbox_contract::CommandOutputChunk;

use super::ChunkSink;

/// Drops every chunk. Used when the request did not ask for streaming.
pub struct DiscardChunks;

#[async_trait]
impl ChunkSink for DiscardChunks {
    async fn publish(&self, _chunk: CommandOutputChunk) {}
}
