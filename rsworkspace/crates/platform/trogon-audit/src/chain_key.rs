use crate::chain_id::ChainId;
use crate::writer_id::WriterId;

/// Names one chain: the writer and the chain it began at its start. Sequences
/// restart under the same writer id, so neither half names a chain alone
/// (ADR#0086 Decision 6).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChainKey {
    writer: WriterId,
    chain: ChainId,
}

impl ChainKey {
    pub fn new(writer: WriterId, chain: ChainId) -> Self {
        Self { writer, chain }
    }

    pub fn writer(&self) -> &WriterId {
        &self.writer
    }

    pub fn chain(&self) -> ChainId {
        self.chain
    }
}

impl std::fmt::Display for ChainKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.writer, self.chain)
    }
}

#[cfg(test)]
mod tests;
