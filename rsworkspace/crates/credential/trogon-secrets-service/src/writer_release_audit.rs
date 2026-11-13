use trogon_audit::{AuditSink, AuditWriter, Disposition, Record, Stamper};
use trogon_std::NowV7;
use trogon_std::time::{EpochClock, SystemClock};
use trogon_std::uuid::UuidV7Generator;

use super::release_audit_records::{denial_record, release_record};
use super::{DenialRecord, ReleaseAudit, ReleaseAuditError, ReleaseRecord};

/// The release core's audit trail on the platform audit writer.
///
/// A release is a must-audit credential use: it is acknowledged only once the
/// writer reports it stored. A denial is a deny decision that stands: once the
/// writer holds it for retry it is acknowledged, and only a writer that has
/// stopped loses it.
///
/// The caller keeps a clone of the writer to run its drain loop.
pub struct WriterReleaseAudit<S, C = SystemClock, G = UuidV7Generator> {
    writer: AuditWriter<S>,
    stamper: Stamper<C, G>,
}

impl<S: AuditSink> WriterReleaseAudit<S> {
    pub fn new(writer: AuditWriter<S>) -> Self {
        Self::with_stamper(writer, Stamper::new(SystemClock, UuidV7Generator))
    }
}

impl<S, C, G> WriterReleaseAudit<S, C, G>
where
    S: AuditSink,
    C: EpochClock,
    G: NowV7 + Send + Sync + 'static,
{
    pub fn with_stamper(writer: AuditWriter<S>, stamper: Stamper<C, G>) -> Self {
        Self { writer, stamper }
    }

    /// Writes on its own task, so a record the writer has numbered is still
    /// stored, or held for retry, when the request that asked for it is dropped.
    /// The record is acknowledged only when the writer kept it in a way `kept`
    /// accepts.
    async fn write(&self, record: Record, kept: &[Disposition]) -> Result<(), ReleaseAuditError> {
        let writer = self.writer.clone();
        let disposition = tokio::spawn(async move { writer.write(record).await })
            .await
            .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
            .map_err(|error| ReleaseAuditError::new(error.to_string()))?;
        if kept.contains(&disposition) {
            Ok(())
        } else {
            Err(not_stored(disposition))
        }
    }
}

impl<S, C, G> ReleaseAudit for WriterReleaseAudit<S, C, G>
where
    S: AuditSink,
    C: EpochClock,
    G: NowV7 + Send + Sync + 'static,
{
    async fn record_release(&self, record: &ReleaseRecord) -> Result<(), ReleaseAuditError> {
        let record = release_record(record, self.stamper.stamp()).map_err(unrecordable)?;
        self.write(record, &[Disposition::Stored]).await
    }

    async fn record_denial(&self, record: &DenialRecord) -> Result<(), ReleaseAuditError> {
        let record = denial_record(record, self.stamper.stamp()).map_err(unrecordable)?;
        self.write(record, &[Disposition::Stored, Disposition::Retrying]).await
    }
}

fn unrecordable(error: super::ReleaseAuditRecordError) -> ReleaseAuditError {
    ReleaseAuditError::new(error.to_string())
}

fn not_stored(disposition: Disposition) -> ReleaseAuditError {
    ReleaseAuditError::new(format!("the audit writer did not keep the record: {disposition:?}"))
}

#[cfg(test)]
mod tests;
