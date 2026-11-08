use std::future::Future;

use crate::signed_checkpoint::SignedCheckpoint;
use crate::sink_error::SinkError;
use crate::stored_ack::StoredAck;

/// Where signed checkpoints go: the `AUDIT_CHECKPOINT` stream. `Ok` means the
/// checkpoint is stored, and the ack says where.
///
/// It is a port of its own beside [`AuditSink`](crate::AuditSink) because a
/// checkpoint is not a record: it has no place in a chain and nothing waits on
/// it.
pub trait CheckpointSink: Clone + Send + Sync + 'static {
    fn publish_checkpoint(
        &self,
        checkpoint: &SignedCheckpoint,
    ) -> impl Future<Output = Result<StoredAck, SinkError>> + Send;
}
