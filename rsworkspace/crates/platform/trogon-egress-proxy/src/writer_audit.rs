//! The proxy's audit sink: each decision becomes a platform audit record,
//! handed to a [`trogon_audit::AuditWriter`], and one local `tracing` line.
//!
//! Whether a write may fail the connection is not decided here. The writer
//! reads it off the record: a permit is acknowledged before the tunnel opens or
//! the call returns `Err`, and a refusal is buffered and retried so it stands
//! whether or not the stream answers (ADR#0086 Decision 4).

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use trogon_audit::{AuditName, AuditSink as Sink, AuditWriter, Disposition, Stamper};
use trogon_nats::NatsToken;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

use crate::audit::{AuditEvent, AuditSink, Decision};
use crate::audit_error::AuditError;
use crate::constants::AUDIT_LOG_TARGET;
use crate::decision_record::decision_record;
use crate::fault::Fault;

pub struct WriterAudit<S> {
    writer: AuditWriter<S>,
    stamper: Stamper<SystemClock, UuidV7Generator>,
    tenant: AuditName,
    project: NatsToken,
}

impl<S: Sink> WriterAudit<S> {
    pub fn new(writer: AuditWriter<S>, tenant: AuditName, project: NatsToken) -> Self {
        Self {
            writer,
            stamper: Stamper::new(SystemClock, UuidV7Generator),
            tenant,
            project,
        }
    }

    async fn write(&self, event: &AuditEvent<'_>) -> Result<Disposition, AuditError> {
        let record = decision_record(event, &self.tenant, &self.project, self.stamper.stamp())?;
        Ok(self.writer.write(record).await?)
    }
}

#[async_trait]
impl<S: Sink> AuditSink for WriterAudit<S> {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
        let outcome = self.write(event).await;
        log(event, &outcome);
        outcome.map(|_| ())
    }

    async fn run(&self, shutdown: CancellationToken) {
        self.writer.run(shutdown).await;
    }
}

/// The one local line per decision, for operators who read logs rather than the
/// stream. A permit whose record failed is logged as the refusal it became.
/// Nothing from the request but the names below reaches it.
fn log(event: &AuditEvent<'_>, outcome: &Result<Disposition, AuditError>) {
    let decision = match (event.decision, outcome) {
        (Decision::Permitted, Err(_)) => Decision::Refused(Fault::AuditUnavailable),
        (decision, _) => decision,
    };
    let destination = event.destination.map(ToString::to_string);
    let resolved = event.resolved_address.map(|address| address.to_string());
    let (audit, failure) = match outcome {
        Ok(disposition) => (disposition_name(*disposition), "-"),
        Err(AuditError::Unavailable(error)) => ("unavailable", error.reason().as_str()),
        Err(AuditError::Name(_) | AuditError::Reason(_)) => ("unavailable", "unrecordable"),
    };
    tracing::info!(
        target: AUDIT_LOG_TARGET,
        agent = %event.agent,
        destination = destination.as_deref().unwrap_or("-"),
        resolved_address = resolved.as_deref().unwrap_or("-"),
        decision = decision.as_str(),
        fault = decision.fault().map_or("-", Fault::code),
        snapshot_source = %event.snapshot_source,
        audit,
        audit_failure = failure,
        "egress decision"
    );
}

fn disposition_name(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Stored => "stored",
        Disposition::Retrying => "retrying",
        Disposition::Queued => "queued",
        Disposition::Dropped => "dropped",
    }
}

#[cfg(test)]
mod tests;
