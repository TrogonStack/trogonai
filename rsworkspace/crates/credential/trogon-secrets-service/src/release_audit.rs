use std::future::Future;

use super::{DenialRecord, ReleaseAuditError, ReleaseRecord};

/// The audit trail of release decisions.
///
/// A release is recorded before the value is sealed, and only an acknowledged
/// record lets the value leave. A denial is recorded as a decision that stands:
/// its write failing never turns it into a release.
pub trait ReleaseAudit: Send + Sync + 'static {
    /// Records a release and returns once the record is durable.
    fn record_release(&self, record: &ReleaseRecord) -> impl Future<Output = Result<(), ReleaseAuditError>> + Send;

    /// Records a denial.
    fn record_denial(&self, record: &DenialRecord) -> impl Future<Output = Result<(), ReleaseAuditError>> + Send;
}
