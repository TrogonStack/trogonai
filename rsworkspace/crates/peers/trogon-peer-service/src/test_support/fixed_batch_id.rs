use crate::{BatchIdGenerator, PeerPortError};
use trogon_peer::BatchId;

#[derive(Clone)]
pub struct FixedBatchId(pub BatchId);

impl BatchIdGenerator for FixedBatchId {
    fn batch_id(&self) -> Result<BatchId, PeerPortError> {
        Ok(self.0.clone())
    }
}
