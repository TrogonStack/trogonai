//! The writer that stands between an enforcing point and its audit stream
//! (ADR#0086 Decisions 4 and 6).
//!
//! One entry point, [`AuditWriter::write`], and the record decides the rest. A
//! must-audit record of an act not yet done is acknowledged before the effect or
//! the act is refused. A refusal stands, so its record goes to a bounded retry
//! buffer when the stream will not take it. A best-effort record goes to a bounded
//! queue and is published asynchronously. The caller picks none of this.
//!
//! Every record the writer stores is a link of one hash chain. A writer opens a
//! fresh chain with a chain reset record at sequence 1, and numbers a record only
//! when it is the next to publish and nothing numbered is still unacknowledged.
//! A record is dropped only before it is numbered, and a numbered record is
//! retried with the same bytes until it is stored, so the stored chain has no gap.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use buffa::Message;
use tokio::sync::{Notify, RwLock, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};
use trogon_std::NowV7;
use trogon_std::time::EpochClock;
use trogonai_proto::platform::audit::v1::AuditClass;

use crate::audit_sink::AuditSink;
use crate::audit_unavailable_error::AuditUnavailableError;
use crate::chain_head::ChainHead;
use crate::chain_id::ChainId;
use crate::chain_key::ChainKey;
use crate::chain_tip::ChainTip;
use crate::constants::{GENESIS_SEQUENCE, LOG_TARGET, MAX_RECORD_BYTES};
use crate::disposition::Disposition;
use crate::failure_reason::FailureReason;
use crate::head_watch::HeadWatch;
use crate::obligation::Obligation;
use crate::record::Record;
use crate::record_id::RecordId;
use crate::sealed_record::SealedRecord;
use crate::stamper::Stamper;
use crate::stored_ack::StoredAck;
use crate::writer_config::WriterConfig;
use crate::writer_identity::WriterIdentity;
use crate::writer_state::WriterState;
use crate::writer_stats::WriterStats;

#[derive(Debug)]
pub struct AuditWriter<S> {
    inner: Arc<Inner<S>>,
}

#[derive(Debug)]
struct Inner<S> {
    sink: S,
    identity: WriterIdentity,
    chain: ChainId,
    config: WriterConfig,
    /// The chain gate. Numbering and publishing a record happen under it, so the
    /// stream receives the chain in order and one record at a time. Waiters take
    /// it in arrival order.
    tip: tokio::sync::Mutex<ChainTip>,
    /// The final drain takes this exclusively, after every admitted producer
    /// and drainer has finished, so its acknowledged head cannot move again.
    admission: RwLock<()>,
    state: Mutex<WriterState>,
    wake: Notify,
    /// The last acknowledged link, published so a checkpoint task can follow the
    /// head without taking the chain gate.
    heads: watch::Sender<Option<ChainHead>>,
    /// Cancelled once the drain task has made its final drain, so nothing moves
    /// the head after it.
    stopped: CancellationToken,
}

impl<S> Clone for AuditWriter<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S: AuditSink> AuditWriter<S> {
    /// A writer that opens a fresh chain under `identity`, its chain id and its
    /// chain reset record taken from `stamper`. Every must-audit record waits for
    /// the chain reset to be stored, and the drain task stores it as soon as it
    /// runs.
    pub fn new<C: EpochClock, G: NowV7>(
        sink: S,
        identity: WriterIdentity,
        config: WriterConfig,
        stamper: &Stamper<C, G>,
    ) -> Self {
        let chain = stamper.chain_id();
        let reset = Record::chain_reset(&identity, stamper.stamp());
        let genesis = SealedRecord::seal(reset, identity.writer(), chain, GENESIS_SEQUENCE, None);
        let writer = Self {
            inner: Arc::new(Inner {
                sink,
                identity,
                chain,
                config,
                tip: tokio::sync::Mutex::new(ChainTip::opened_by(genesis)),
                admission: RwLock::new(()),
                state: Mutex::new(WriterState::default()),
                wake: Notify::new(),
                heads: watch::Sender::new(None),
                stopped: CancellationToken::new(),
            }),
        };
        writer.inner.wake.notify_one();
        writer
    }

    /// Accepts a record. `Err` is returned only for a must-audit record of an act
    /// that has not happened yet, and means the act must be refused as an
    /// operational fault. A record refused after it was numbered stays the head of
    /// the chain and is stored by a later retry, so the stream can hold the record
    /// of a decision whose act was refused, a limit ADR#0086 states. A refusal's
    /// record and a best-effort record never fail the caller.
    /// After the final drain, effects are refused and other records are dropped
    /// and counted, so no write can move a head whose final checkpoint was taken.
    pub async fn write(&self, record: Record) -> Result<Disposition, AuditUnavailableError> {
        let _admission = self.inner.admission.read().await;
        if self.inner.stopped.is_cancelled() {
            return self.closed(&record);
        }
        if !fits_a_message(&record) {
            return self.too_large(&record);
        }
        match record.obligation() {
            Obligation::BeforeEffect => self.write_before_effect(record).await,
            Obligation::Stands => Ok(self.write_standing_refusal(record).await),
            Obligation::BestEffort => Ok(self.enqueue_best_effort(record)),
        }
    }

    /// True while the chain's head waits to be stored after a failed publish, or
    /// while the retry buffer has been full and has not yet emptied. A degraded
    /// writer refuses every action that needs a must-audit record before an effect
    /// and keeps denying what it denies.
    pub fn is_degraded(&self) -> bool {
        self.inner.stopped.is_cancelled() || self.state().degraded()
    }

    pub fn stats(&self) -> WriterStats {
        let mut stats = self.state().stats();
        stats.degraded |= self.inner.stopped.is_cancelled();
        stats
    }

    /// The last record the stream acknowledged, or `None` until the chain reset is
    /// stored.
    pub fn head(&self) -> Option<ChainHead> {
        self.inner.heads.borrow().clone()
    }

    /// The chain this writer instance appends to.
    pub fn chain(&self) -> ChainId {
        self.inner.chain
    }

    /// Follows this writer's head from outside the chain gate, for a checkpoint
    /// task.
    pub(crate) fn head_watch(&self) -> HeadWatch {
        HeadWatch::new(
            ChainKey::new(self.inner.identity.writer().clone(), self.inner.chain),
            self.inner.identity.component().clone(),
            self.inner.heads.subscribe(),
            self.inner.stopped.clone(),
        )
    }

    /// Stores the chain's head if it is waiting, then numbers and stores the
    /// buffers one record at a time: the retry buffer first and in order, then the
    /// best-effort queue. The drain stops at the first failure and leaves that
    /// record at the head, so a stream that is down costs one publish per pass.
    /// The writer leaves audit-degraded once the head is stored and the retry
    /// buffer is empty.
    /// After the final drain this is a no-op: a recovered sink must not advance a
    /// head after the checkpoint task has stopped following it.
    pub async fn drain_once(&self) {
        let _admission = self.inner.admission.read().await;
        if self.inner.stopped.is_cancelled() {
            return;
        }
        self.drain().await;
    }

    async fn drain(&self) {
        loop {
            let mut tip = self.inner.tip.lock().await;
            if self.settle(&mut tip).await.is_err() {
                return;
            }
            let Some((record, retried)) = self.next_buffered() else {
                return;
            };
            if let Err(record) = self.append(&mut tip, record) {
                self.put_back(*record, retried);
                return;
            }
        }
    }

    /// Drains on every best-effort enqueue and every `drain_interval` until
    /// `shutdown`, then once more so a clean stop flushes what it can. A
    /// checkpoint task following this writer stops after that final drain.
    /// The final drain fences new producers and waits for admitted writes before
    /// it closes the writer and announces a stable head to the checkpoint task.
    pub async fn run(&self, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = self.inner.wake.notified() => {}
                () = tokio::time::sleep(self.inner.config.drain_interval) => {}
            }
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = self.drain_once() => {}
            }
        }
        let _admission = self.inner.admission.write().await;
        if self.inner.stopped.is_cancelled() {
            return;
        }
        self.drain().await;
        self.inner.stopped.cancel();
    }

    pub fn spawn_drain(&self, shutdown: CancellationToken) -> JoinHandle<()> {
        let writer = self.clone();
        tokio::spawn(async move { writer.run(shutdown).await })
    }

    fn state(&self) -> MutexGuard<'_, WriterState> {
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn too_large(&self, record: &Record) -> Result<Disposition, AuditUnavailableError> {
        let class = record.kind().audit_class();
        if record.obligation() == Obligation::BeforeEffect {
            return Err(self.refuse(record.id(), class, FailureReason::TooLarge));
        }
        self.state().count_drop(class, FailureReason::TooLarge);
        error!(
            target: LOG_TARGET,
            record_id = %record.id(),
            ?class,
            reason = %FailureReason::TooLarge,
            "an audit record is larger than the stream takes and was dropped"
        );
        Ok(Disposition::Dropped)
    }

    fn closed(&self, record: &Record) -> Result<Disposition, AuditUnavailableError> {
        let class = record.kind().audit_class();
        if record.obligation() == Obligation::BeforeEffect {
            return Err(self.refuse(record.id(), class, FailureReason::Closed));
        }
        self.state().count_drop(class, FailureReason::Closed);
        error!(
            target: LOG_TARGET,
            record_id = %record.id(),
            ?class,
            reason = %FailureReason::Closed,
            "the audit writer has stopped, a record was dropped"
        );
        Ok(Disposition::Dropped)
    }

    async fn write_before_effect(&self, record: Record) -> Result<Disposition, AuditUnavailableError> {
        let (id, class) = (record.id(), record.kind().audit_class());
        if self.is_degraded() {
            return Err(self.refuse(id, class, FailureReason::Degraded));
        }
        let mut tip = self.inner.tip.lock().await;
        if self.is_degraded() {
            return Err(self.refuse(id, class, FailureReason::Degraded));
        }
        if let Err(reason) = self.settle(&mut tip).await {
            return Err(self.refuse(id, class, reason));
        }
        if self.append(&mut tip, record).is_err() {
            return Err(self.refuse(id, class, FailureReason::Degraded));
        }
        match self.settle(&mut tip).await {
            Ok(()) => Ok(Disposition::Stored),
            Err(reason) => Err(self.refuse(id, class, reason)),
        }
    }

    fn refuse(&self, id: RecordId, class: AuditClass, reason: FailureReason) -> AuditUnavailableError {
        self.state().count_refusal(reason);
        warn!(
            target: LOG_TARGET,
            record_id = %id,
            ?class,
            %reason,
            "audit unavailable, refusing the action"
        );
        AuditUnavailableError::new(reason)
    }

    /// A refusal takes effect whether or not its record is written. The record is
    /// published inline only when nothing waits ahead of it and the writer is not
    /// degraded, so a refusal does not wait on a stream that is already failing
    /// and is never stored ahead of a refusal still in the buffer. Once its own
    /// publish has started it is the chain's head and is retried from there.
    async fn write_standing_refusal(&self, record: Record) -> Disposition {
        if self.must_buffer() {
            return self.buffer_refusal(record);
        }
        let mut tip = self.inner.tip.lock().await;
        if self.must_buffer() || self.settle(&mut tip).await.is_err() {
            return self.buffer_refusal(record);
        }
        if let Err(record) = self.append(&mut tip, record) {
            return self.buffer_refusal(*record);
        }
        match self.settle(&mut tip).await {
            Ok(()) => Disposition::Stored,
            Err(_) => Disposition::Retrying,
        }
    }

    fn must_buffer(&self) -> bool {
        let state = self.state();
        state.degraded() || !state.retry.is_empty()
    }

    fn buffer_refusal(&self, record: Record) -> Disposition {
        let capacity = self.inner.config.retry_capacity.max(1);
        let mut state = self.state();
        if state.retry.len() >= capacity {
            let class = record.kind().audit_class();
            state.overflowed = true;
            state.count_drop(class, FailureReason::BufferFull);
            error!(
                target: LOG_TARGET,
                record_id = %record.id(),
                ?class,
                reason = %FailureReason::BufferFull,
                "audit retry buffer is full, a must-audit record was dropped"
            );
            return Disposition::Dropped;
        }
        state.retry.push_back(record);
        if state.retry.len() >= capacity {
            state.overflowed = true;
            warn!(target: LOG_TARGET, "audit retry buffer is full, the writer is audit-degraded");
        }
        Disposition::Retrying
    }

    fn enqueue_best_effort(&self, record: Record) -> Disposition {
        let capacity = self.inner.config.best_effort_capacity.max(1);
        let mut state = self.state();
        if state.queue.len() >= capacity {
            state.count_drop(AuditClass::BestEffort, FailureReason::QueueFull);
            warn!(
                target: LOG_TARGET,
                record_id = %record.id(),
                reason = %FailureReason::QueueFull,
                "a best-effort audit record was dropped"
            );
            return Disposition::Dropped;
        }
        state.queue.push_back(record);
        drop(state);
        self.inner.wake.notify_one();
        Disposition::Queued
    }

    /// The next record to number, and whether it came from the retry buffer. The
    /// retry buffer empties here, which ends the overflow it may have raised.
    fn next_buffered(&self) -> Option<(Record, bool)> {
        let mut state = self.state();
        if let Some(record) = state.retry.pop_front() {
            if state.retry.is_empty() {
                state.overflowed = false;
            }
            return Some((record, true));
        }
        state.queue.pop_front().map(|record| (record, false))
    }

    fn put_back(&self, record: Record, retried: bool) {
        let mut state = self.state();
        if retried {
            state.retry.push_front(record);
        } else {
            state.queue.push_front(record);
        }
    }

    /// Numbers `record` as the chain's next link and parks it as the head before
    /// anything awaits its publish, so a caller that stops waiting leaves it for
    /// the next holder of the gate or the drain. A chain out of sequences blocks.
    fn append(&self, tip: &mut ChainTip, record: Record) -> Result<(), Box<Record>> {
        tip.append(record, self.inner.identity.writer(), self.inner.chain)
            .inspect_err(|record| {
                self.state().blocked = true;
                error!(
                    target: LOG_TARGET,
                    record_id = %record.id(),
                    "the audit chain has no sequence left, the writer is audit-degraded"
                );
            })
    }

    /// Publishes the chain's head if one waits. On success the head becomes the
    /// acknowledged position; on failure it stays, blocking the chain, and is
    /// retried with the same bytes and message id.
    async fn settle(&self, tip: &mut ChainTip) -> Result<(), FailureReason> {
        let Some(pending) = tip.pending.as_mut() else {
            return Ok(());
        };
        let first_attempt = *pending.first_attempt.get_or_insert_with(Instant::now);
        match self.inner.sink.publish(&pending.record).await {
            Ok(ack) => {
                self.stored(&pending.record, first_attempt, &ack);
                tip.pending = None;
                Ok(())
            }
            Err(error) => {
                let reason = error.reason();
                self.state().blocked = true;
                warn!(
                    target: LOG_TARGET,
                    record_id = %pending.record.id(),
                    sequence = pending.record.sequence(),
                    %reason,
                    "the audit chain head could not be stored"
                );
                Err(reason)
            }
        }
    }

    fn stored(&self, record: &SealedRecord, first_attempt: Instant, ack: &StoredAck) {
        self.inner.heads.send_replace(Some(ChainHead::new(
            record.sequence(),
            record.hash(),
            ack.stream(),
            ack.stream_sequence(),
        )));
        let mut state = self.state();
        state.blocked = false;
        if !ack.duplicate() && first_attempt.elapsed() > self.inner.config.duplicate_window {
            state.possible_duplicates += 1;
            warn!(
                target: LOG_TARGET,
                record_id = %record.id(),
                sequence = record.sequence(),
                "an audit record was stored by a retry after the duplicate window, the stream may hold it twice"
            );
        }
    }
}

/// Whether the record, once sealed, fits in one message the stream takes.
fn fits_a_message(record: &Record) -> bool {
    record
        .message()
        .try_encoded_len()
        .ok()
        .and_then(|len| usize::try_from(len).ok())
        .is_some_and(|len| len <= MAX_RECORD_BYTES)
}

#[cfg(test)]
mod tests;
