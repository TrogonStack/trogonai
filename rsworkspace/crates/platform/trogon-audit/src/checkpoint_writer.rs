//! The task that signs a writer's acknowledged head and stores it on
//! `AUDIT_CHECKPOINT` (ADR#0086 Decision 6).
//!
//! A chain alone cannot show that its tail was deleted, because the records left
//! still link. A signed checkpoint names a head, so a verifier holding one knows
//! the chain reached at least that far.
//!
//! The task follows the head from outside the chain gate and never holds up a
//! record: a failed signature or publish is logged as an alert and counted, the
//! writer keeps its readiness, and the next checkpoint covers a later head.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::error;
use trogon_std::NowV7;
use trogon_std::time::EpochClock;

use crate::audit_sink::AuditSink;
use crate::audit_writer::AuditWriter;
use crate::chain_head::ChainHead;
use crate::checkpoint_config::CheckpointConfig;
use crate::checkpoint_draft::CheckpointDraft;
use crate::checkpoint_sign_error::CheckpointSignError;
use crate::checkpoint_signer::CheckpointSigner;
use crate::checkpoint_sink::CheckpointSink;
use crate::checkpoint_stats::CheckpointStats;
use crate::constants::LOG_TARGET;
use crate::head_watch::HeadWatch;
use crate::signed_checkpoint::SignedCheckpoint;
use crate::sink_error::SinkError;
use crate::stamper::Stamper;

#[derive(Debug)]
pub struct CheckpointWriter<S, C, G> {
    inner: Arc<Inner<S, C, G>>,
}

#[derive(Debug)]
struct Inner<S, C, G> {
    sink: S,
    signer: Arc<dyn CheckpointSigner>,
    config: CheckpointConfig,
    stamper: Stamper<C, G>,
    watch: HeadWatch,
    stats: Mutex<CheckpointStats>,
}

impl<S, C, G> Clone for CheckpointWriter<S, C, G> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S, C, G> CheckpointWriter<S, C, G>
where
    S: CheckpointSink,
    C: EpochClock + Send + Sync + 'static,
    G: NowV7 + Send + Sync + 'static,
{
    /// A task that checkpoints `writer`'s chain, signing with `signer` and taking
    /// each checkpoint's id and issue time from `stamper`.
    pub fn new<A: AuditSink>(
        writer: &AuditWriter<A>,
        sink: S,
        signer: Arc<dyn CheckpointSigner>,
        config: CheckpointConfig,
        stamper: Stamper<C, G>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                signer,
                config,
                stamper,
                watch: writer.head_watch(),
                stats: Mutex::new(CheckpointStats::default()),
            }),
        }
    }

    pub fn stats(&self) -> CheckpointStats {
        *self.stats_mut()
    }

    /// Checkpoints the first head the writer stores, which is its chain reset,
    /// then the head after every `every_records` more records, and the current
    /// head again whenever `interval` passes without a checkpoint, so an idle
    /// chain still has a recent one. The schedule restarts from every attempt,
    /// failed or not, so a failing signer or stream costs one attempt per trigger.
    ///
    /// Runs until the writer's drain task has made its final drain, or until the
    /// writer is dropped, and then checkpoints the head it was left at.
    /// Signing and publication each have a configured timeout, including that
    /// final attempt, so a pending adapter cannot keep the task alive forever.
    pub async fn run(&self) {
        let watch = &self.inner.watch;
        let mut heads = watch.heads.clone();
        heads.mark_changed();
        let mut checkpointed: Option<u64> = None;
        let mut deadline = Instant::now() + self.inner.config.interval.into();
        loop {
            let head = tokio::select! {
                biased;
                () = watch.stopped.cancelled() => break,
                changed = heads.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    heads.borrow_and_update().clone().filter(|head| self.due(checkpointed, head))
                }
                () = tokio::time::sleep_until(deadline) => {
                    deadline = Instant::now() + self.inner.config.interval.into();
                    heads.borrow_and_update().clone()
                }
            };
            if let Some(head) = head {
                checkpointed = Some(head.sequence());
                self.checkpoint(head).await;
                deadline = Instant::now() + self.inner.config.interval.into();
            }
        }
        let last = heads.borrow().clone();
        if let Some(head) = last {
            self.checkpoint(head).await;
        }
    }

    pub fn spawn(&self) -> JoinHandle<()> {
        let writer = self.clone();
        tokio::spawn(async move { writer.run().await })
    }

    fn due(&self, checkpointed: Option<u64>, head: &ChainHead) -> bool {
        checkpointed
            .is_none_or(|sequence| head.sequence().saturating_sub(sequence) >= self.inner.config.every_records.get())
    }

    /// Signs and stores one checkpoint of `head`, counting the outcome.
    async fn checkpoint(&self, head: ChainHead) {
        let watch = &self.inner.watch;
        let sequence = head.sequence();
        let draft = CheckpointDraft::new(
            self.inner.stamper.checkpoint_id(),
            watch.key.clone(),
            watch.component.clone(),
            head,
            self.inner.stamper.now(),
        );
        let signature = tokio::time::timeout(self.inner.config.sign_timeout.into(), self.inner.signer.sign(&draft))
            .await
            .unwrap_or(Err(CheckpointSignError::Timeout));
        let signed = match signature {
            Ok(signature) => SignedCheckpoint::assemble(&draft, &signature),
            Err(error) => Err(error),
        };
        let checkpoint = match signed {
            Ok(checkpoint) => checkpoint,
            Err(reason) => {
                self.stats_mut().sign_failures += 1;
                error!(
                    target: LOG_TARGET,
                    chain = %watch.key,
                    sequence,
                    reason = reason.as_str(),
                    "an audit checkpoint could not be signed"
                );
                return;
            }
        };
        let published = tokio::time::timeout(
            self.inner.config.publish_timeout.into(),
            self.inner.sink.publish_checkpoint(&checkpoint),
        )
        .await
        .unwrap_or(Err(SinkError::Timeout));
        match published {
            Ok(_) => self.stats_mut().written += 1,
            Err(error) => {
                self.stats_mut().publish_failures += 1;
                error!(
                    target: LOG_TARGET,
                    chain = %watch.key,
                    sequence,
                    checkpoint_id = %checkpoint.id(),
                    reason = %error.reason(),
                    "an audit checkpoint could not be stored"
                );
            }
        }
    }

    fn stats_mut(&self) -> MutexGuard<'_, CheckpointStats> {
        self.inner.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
