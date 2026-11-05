//! The writer that stands between an enforcing point and its audit stream
//! (ADR#0086 Decision 4).
//!
//! One entry point, [`AuditWriter::write`], and the record decides the rest. A
//! must-audit record of an act not yet done is acknowledged before the effect or
//! the act is refused. A refusal stands, so its record goes to a bounded retry
//! buffer when the stream will not take it. A best-effort record goes to a bounded
//! queue and is published asynchronously. The caller picks none of this.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};
use trogonai_proto::platform::audit::v1::AuditClass;

use crate::audit_sink::AuditSink;
use crate::audit_unavailable_error::AuditUnavailableError;
use crate::constants::LOG_TARGET;
use crate::disposition::Disposition;
use crate::failure_reason::FailureReason;
use crate::obligation::Obligation;
use crate::record::Record;
use crate::sealed_record::SealedRecord;
use crate::writer_config::WriterConfig;
use crate::writer_id::WriterId;
use crate::writer_state::WriterState;
use crate::writer_stats::WriterStats;

#[derive(Debug)]
pub struct AuditWriter<S> {
    inner: Arc<Inner<S>>,
}

#[derive(Debug)]
struct Inner<S> {
    sink: S,
    writer: WriterId,
    config: WriterConfig,
    sequence: AtomicU64,
    state: Mutex<WriterState>,
    wake: Notify,
    drain_gate: tokio::sync::Mutex<()>,
}

impl<S> Clone for AuditWriter<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S: AuditSink> AuditWriter<S> {
    pub fn new(sink: S, writer: WriterId, config: WriterConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                writer,
                config,
                sequence: AtomicU64::new(0),
                state: Mutex::new(WriterState::default()),
                wake: Notify::new(),
                drain_gate: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Accepts a record. `Err` is returned only for a must-audit record of an act
    /// that has not happened yet, and means the act must be refused as an
    /// operational fault: it is not stored and no record of the refused attempt
    /// exists. A refusal's record and a best-effort record never fail the caller.
    pub async fn write(&self, record: Record) -> Result<Disposition, AuditUnavailableError> {
        let obligation = record.obligation();
        let sealed = self.seal(record);
        match obligation {
            Obligation::BeforeEffect => self.write_before_effect(sealed).await,
            Obligation::Stands => Ok(self.write_standing_refusal(sealed).await),
            Obligation::BestEffort => Ok(self.enqueue_best_effort(sealed)),
        }
    }

    /// True while the retry buffer has been full and has not yet drained. A degraded
    /// writer refuses every action that needs a must-audit record before an effect
    /// and keeps denying what it denies.
    pub fn is_degraded(&self) -> bool {
        self.state().degraded
    }

    pub fn stats(&self) -> WriterStats {
        self.state().stats()
    }

    /// Retries the buffers once: the retry buffer first and in order, then the
    /// best-effort queue. A refusal's record leaves the buffer only when it is
    /// acknowledged, and the writer leaves audit-degraded when the buffer is
    /// empty.
    pub async fn drain_once(&self) {
        let _serialized = self.inner.drain_gate.lock().await;
        self.drain_retry_buffer().await;
        self.drain_best_effort_queue().await;
    }

    /// Drains on every best-effort enqueue and every `drain_interval` until
    /// `shutdown`, then once more so a clean stop flushes what it can.
    pub async fn run(&self, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = self.inner.wake.notified() => {}
                () = tokio::time::sleep(self.inner.config.drain_interval) => {}
            }
            self.drain_once().await;
        }
        self.drain_once().await;
    }

    pub fn spawn_drain(&self, shutdown: CancellationToken) -> JoinHandle<()> {
        let writer = self.clone();
        tokio::spawn(async move { writer.run(shutdown).await })
    }

    fn state(&self) -> MutexGuard<'_, WriterState> {
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn seal(&self, record: Record) -> SealedRecord {
        let sequence = self.inner.sequence.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        SealedRecord::seal(record, &self.inner.writer, sequence)
    }

    async fn write_before_effect(&self, record: SealedRecord) -> Result<Disposition, AuditUnavailableError> {
        if self.is_degraded() {
            return Err(self.refuse(&record, FailureReason::Degraded));
        }
        match self.inner.sink.publish(&record).await {
            Ok(()) => Ok(Disposition::Stored),
            Err(error) => Err(self.refuse(&record, error.reason())),
        }
    }

    fn refuse(&self, record: &SealedRecord, reason: FailureReason) -> AuditUnavailableError {
        self.state().count_refusal(reason);
        warn!(
            target: LOG_TARGET,
            record_id = %record.id(),
            class = ?record.class(),
            %reason,
            "audit unavailable, refusing the action"
        );
        AuditUnavailableError::new(reason)
    }

    /// A refusal takes effect whether or not its record is written. The record is
    /// published inline only when nothing is waiting ahead of it, so ordering holds
    /// and a refusal does not wait on a stream that is already failing.
    async fn write_standing_refusal(&self, record: SealedRecord) -> Disposition {
        let nothing_ahead = self.state().retry.is_empty();
        if nothing_ahead && self.inner.sink.publish(&record).await.is_ok() {
            return Disposition::Stored;
        }
        self.buffer_refusal(record)
    }

    fn buffer_refusal(&self, record: SealedRecord) -> Disposition {
        let capacity = self.inner.config.retry_capacity.max(1);
        let mut state = self.state();
        if state.retry.len() >= capacity {
            state.degraded = true;
            state.count_drop(record.class(), FailureReason::BufferFull);
            error!(
                target: LOG_TARGET,
                record_id = %record.id(),
                class = ?record.class(),
                reason = %FailureReason::BufferFull,
                "audit retry buffer is full, a must-audit record was dropped"
            );
            return Disposition::Dropped;
        }
        state.retry.push_back(record);
        if state.retry.len() >= capacity {
            state.degraded = true;
            warn!(target: LOG_TARGET, "audit retry buffer is full, the writer is audit-degraded");
        }
        Disposition::Retrying
    }

    fn enqueue_best_effort(&self, record: SealedRecord) -> Disposition {
        let capacity = self.inner.config.best_effort_capacity.max(1);
        let mut state = self.state();
        if state.queue.len() >= capacity {
            state.count_drop(AuditClass::BestEffort, FailureReason::QueueFull);
            return Disposition::Dropped;
        }
        state.queue.push_back(record);
        drop(state);
        self.inner.wake.notify_one();
        Disposition::Queued
    }

    async fn drain_retry_buffer(&self) {
        loop {
            let Some(record) = self.state().retry.front().cloned() else {
                return;
            };
            if let Err(error) = self.inner.sink.publish(&record).await {
                warn!(
                    target: LOG_TARGET,
                    record_id = %record.id(),
                    reason = %error.reason(),
                    "audit retry buffer could not be drained"
                );
                return;
            }
            let mut state = self.state();
            state.retry.pop_front();
            if state.retry.is_empty() {
                state.degraded = false;
            }
        }
    }

    async fn drain_best_effort_queue(&self) {
        loop {
            let Some(record) = self.state().queue.pop_front() else {
                return;
            };
            if let Err(error) = self.inner.sink.publish(&record).await {
                self.state().count_drop(AuditClass::BestEffort, error.reason());
                warn!(
                    target: LOG_TARGET,
                    record_id = %record.id(),
                    reason = %error.reason(),
                    "a best-effort audit record was dropped"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
