use crate::chain_outcome::ChainOutcome;

/// Every chain a mixed stream range held, each verified on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainsVerification {
    chains: Vec<ChainOutcome>,
    unattributed: Vec<u64>,
}

impl ChainsVerification {
    pub(crate) fn new(chains: Vec<ChainOutcome>, unattributed: Vec<u64>) -> Self {
        Self { chains, unattributed }
    }

    /// One outcome per chain, in the order each chain first appears in the
    /// stream.
    pub fn chains(&self) -> &[ChainOutcome] {
        &self.chains
    }

    /// Stream sequences of messages that named no readable writer and chain in
    /// their payload or headers, so they belong to no chain a verifier can walk.
    pub fn unattributed(&self) -> &[u64] {
        &self.unattributed
    }

    /// True when every chain verified and every message belonged to one.
    pub fn is_verified(&self) -> bool {
        self.unattributed.is_empty() && self.chains.iter().all(|chain| chain.result().is_ok())
    }
}
