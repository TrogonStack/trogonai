//! The proxy's audit sink: each decision becomes a platform audit record,
//! handed to a [`trogon_audit::AuditWriter`], and one local `tracing` line.
//!
//! Whether a write may fail the connection is not decided here. The writer
//! reads it off the record: a permit is acknowledged before the tunnel opens or
//! the call returns `Err`, and a refusal is buffered and retried so it stands
//! whether or not the stream answers (ADR#0086 Decision 4).
//!
//! Each write runs on a task of its own. Shutdown aborts every connection, and
//! a refusal still being written when its connection goes must reach the retry
//! buffer anyway, so [`WriterAudit::run`] waits for those tasks before the last
//! flush.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use trogon_audit::{
    ActivationRecorder, Actor, AuditName, AuditSink as Sink, AuditWriter, Disposition, RecordScope, Stamper,
};
use trogon_nats::NatsToken;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

use crate::audit::{AuditEvent, AuditSink, Decision};
use crate::audit_error::AuditError;
use crate::connection_limit::ConnectionLimit;
use crate::constants::AUDIT_LOG_TARGET;
use crate::decision_record::decision_record;
use crate::fault::Fault;

pub struct WriterAudit<S> {
    writer: AuditWriter<S>,
    stamper: Stamper<SystemClock, UuidV7Generator>,
    tenant: AuditName,
    project: NatsToken,
    in_flight: TaskTracker,
}

impl<S: Sink> WriterAudit<S> {
    pub fn with_limit(writer: AuditWriter<S>, tenant: AuditName, project: NatsToken, _limit: ConnectionLimit) -> Self {
        Self::new(writer, tenant, project)
    }

    pub(crate) fn activation_recorder(&self) -> ActivationRecorder<S> {
        ActivationRecorder::new(
            self.writer.clone(),
            RecordScope::new(self.tenant.clone(), self.project.clone(), Actor::Unattributed),
        )
    }

    pub fn new(writer: AuditWriter<S>, tenant: AuditName, project: NatsToken) -> Self {
        Self {
            writer,
            stamper: Stamper::new(SystemClock, UuidV7Generator),
            tenant,
            project,
            in_flight: TaskTracker::new(),
        }
    }

    async fn write(&self, event: &AuditEvent<'_>) -> Result<Disposition, AuditError> {
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
impl<S: Sink> AuditSink for WriterAudit<S> {
    async fn record(&self, event: &AuditEvent<'_>) -> Result<(), AuditError> {
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
        Err(AuditError::Overloaded) => ("unavailable", "overloaded"),
        Err(
            AuditError::Name(_) | AuditError::Reason(_) | AuditError::PolicyEvidence | AuditError::WorkloadEvidence(_),
        ) => ("unavailable", "unrecordable"),
    };
    tracing::info!(
        target: AUDIT_LOG_TARGET,
        agent = %event.agent,
        destination = destination.as_deref().unwrap_or("-"),
        resolved_address = resolved.as_deref().unwrap_or("-"),
        decision = decision.as_str(),
        fault = decision.fault().map_or("-", Fault::code),
        snapshot_source = %event.snapshot_source,
        projection_revision = event.policy_revision.map(|revision| revision.generation.get()),
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
