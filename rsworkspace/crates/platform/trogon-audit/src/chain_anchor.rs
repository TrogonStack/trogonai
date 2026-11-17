use crate::chain_id::ChainId;
use crate::verified_chain_head::VerifiedChainHead;
use crate::writer_id::WriterId;

/// Where a verified range starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainAnchor {
    /// The range opens the chain: its first record is sequence 1, has no
    /// previous hash, and is a writer lifecycle record that starts or resets the
    /// chain. The genesis is not signed, so it shows that a chain began and not
    /// that anyone authorized the reset.
    Genesis { writer: WriterId, chain: ChainId },
    /// The range continues right after a head an earlier verification produced.
    TrustedHead(VerifiedChainHead),
}
