//! The gateway's audit sink: each decision becomes a platform audit record,
//! handed to a [`trogon_audit::AuditWriter`], and one local `tracing` line.
//!
//! Whether a write may refuse the request is not decided here. The writer reads
//! it off the record: an allow of a method that is not read-only is acknowledged
//! before it is forwarded or the call returns `Err`, and a deny is buffered and
//! retried so it stands whether or not the stream answers (ADR#0086 Decision 4).
//!
//! Each write runs on a task of its own. Shutdown stops the ingress loop while a
//! deny may still be on its way to the retry buffer, so [`WriterAudit::run`] waits
//! for those tasks before the last flush.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use trogon_audit::{AuditName, AuditSink as Sink, AuditWriter, Disposition, Stamper};
use trogon_nats::NatsToken;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

use super::DecisionAudit;
use super::audit_error::AuditError;
use super::decision_event::{DecisionEvent, Verdict};
use super::decision_record::decision_record;
use crate::constants::AUDIT_LOG_TARGET;

pub struct WriterAudit<S> {
    writer: AuditWriter<S>,
    stamper: Stamper<SystemClock, UuidV7Generator>,
    tenant: AuditName,
    project: NatsToken,
    in_flight: TaskTracker,
}

impl<S: Sink> WriterAudit<S> {
    pub fn new(writer: AuditWriter<S>, tenant: AuditName, project: NatsToken) -> Self {
        Self {
            writer,
            stamper: Stamper::new(SystemClock, UuidV7Generator),
            tenant,
            project,
            in_flight: TaskTracker::new(),
        }
    }

    pub fn writer(&self) -> &AuditWriter<S> {
        &self.writer
    }

    async fn write(&self, event: &DecisionEvent<'_>) -> Result<Disposition, AuditError> {
        let record = decision_record(event, &self.tenant, &self.project, self.stamper.stamp())?;
        let writer = self.writer.clone();
        let write = self.in_flight.spawn(async move { writer.write(record).await });
        let outcome = write
            .await
            .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()));
        Ok(outcome?)
    }
}

#[async_trait]
impl<S: Sink> DecisionAudit for WriterAudit<S> {
    async fn record(&self, event: &DecisionEvent<'_>) -> Result<(), AuditError> {
        let outcome = self.write(event).await;
        log(event, &outcome);
        outcome.map(|_| ())
    }

    async fn run(&self, shutdown: CancellationToken) {
        let settled = CancellationToken::new();
        tokio::join!(self.writer.run(settled.clone()), async {
            shutdown.cancelled().await;
            self.in_flight.close();
            self.in_flight.wait().await;
            settled.cancel();
        });
    }
}

/// The one local line per decision, for operators who read logs rather than the
/// stream. Nothing from the request but the names below reaches it.
fn log(event: &DecisionEvent<'_>, outcome: &Result<Disposition, AuditError>) {
    let decision = match &event.verdict {
        Verdict::Allowed(_) => "allow",
        Verdict::Denied(reason) => reason.code(),
    };
    let (audit, failure) = match outcome {
        Ok(disposition) => (disposition_name(*disposition), "-"),
        Err(AuditError::Unavailable(error)) => ("unavailable", error.reason().as_str()),
        Err(AuditError::Name(_) | AuditError::Reason(_)) => ("unavailable", "unrecordable"),
    };
    tracing::info!(
        target: AUDIT_LOG_TARGET,
        agent = %event.agent_id.as_str(),
        method = event.method,
        decision,
        audit,
        audit_failure = failure,
        "gateway decision"
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
