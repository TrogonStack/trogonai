use crate::chain_key::ChainKey;
use crate::chain_verification_error::ChainVerificationError;
use crate::verified_chain_range::VerifiedChainRange;

/// How one chain of a mixed stream range verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainOutcome {
    key: ChainKey,
    result: Result<VerifiedChainRange, ChainVerificationError>,
}

impl ChainOutcome {
    pub(crate) fn new(key: ChainKey, result: Result<VerifiedChainRange, ChainVerificationError>) -> Self {
        Self { key, result }
    }

    pub fn key(&self) -> &ChainKey {
        &self.key
    }

    pub fn result(&self) -> &Result<VerifiedChainRange, ChainVerificationError> {
        &self.result
    }

    pub fn into_result(self) -> Result<VerifiedChainRange, ChainVerificationError> {
        self.result
    }
}
