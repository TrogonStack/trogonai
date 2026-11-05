use std::future::Future;

use crate::sealed_record::SealedRecord;
use crate::sink_error::SinkError;

/// Where sealed records go. `Ok` means the record is durably stored and
/// acknowledged; anything else means it is not stored.
///
/// A sink retries a lost acknowledgement under the same message id, so a retry
/// cannot store the record twice. The writer, not the sink, decides what a failure
/// means for the action: refusal, buffering or a counted drop.
pub trait AuditSink: Clone + Send + Sync + 'static {
    fn publish(&self, record: &SealedRecord) -> impl Future<Output = Result<(), SinkError>> + Send;
}
