use crate::chain_notice::ChainNotice;
use crate::verified_chain_head::VerifiedChainHead;

/// A chain range that verified, with the head it ends at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedChainRange {
    head: VerifiedChainHead,
    records: usize,
    records_without_metadata: usize,
    notices: Vec<ChainNotice>,
}

impl VerifiedChainRange {
    pub(crate) fn new(
        head: VerifiedChainHead,
        records: usize,
        records_without_metadata: usize,
        notices: Vec<ChainNotice>,
    ) -> Self {
        Self {
            head,
            records,
            records_without_metadata,
            notices,
        }
    }

    pub fn head(&self) -> &VerifiedChainHead {
        &self.head
    }

    pub fn into_head(self) -> VerifiedChainHead {
        self.head
    }

    /// Every stored message the range held, identical duplicates included.
    pub fn records(&self) -> usize {
        self.records
    }

    /// Messages read without their subject or a provenance header. Their payload
    /// and chain links verified, but nothing showed that the stream delivered
    /// them under the subject and headers their writer gave them.
    pub fn records_without_metadata(&self) -> usize {
        self.records_without_metadata
    }

    pub fn notices(&self) -> &[ChainNotice] {
        &self.notices
    }
}
