use std::future::Future;

use crate::sealed_record::SealedRecord;
use crate::sink_error::SinkError;
use crate::stored_ack::StoredAck;

/// Where sealed records go. `Ok` means the record is durably stored, and the ack
/// says where; anything else means it is to be treated as not stored, though a
/// lost acknowledgement can leave it stored.
///
/// A sink retries a lost acknowledgement under the same message id, so a retry
/// inside the stream's duplicate window cannot store the record twice. The
/// writer, not the sink, decides what a failure means for the action: refusal,
/// buffering or a retry of the chain's head.
pub trait AuditSink: Clone + Send + Sync + 'static {
    fn publish(&self, record: &SealedRecord) -> impl Future<Output = Result<StoredAck, SinkError>> + Send;
}
