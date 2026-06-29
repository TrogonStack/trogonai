//! Drives the Postgres schedules projection from the schedule event stream.
//!
//! This is additive and standalone: it does not touch the NATS KV projector or the
//! event store's append path. It reuses the schedules fold state machine
//! ([`crate::projections::schedules::apply`]) so the Postgres read model is
//! identical to the NATS one — event-for-event.
//!
//! Each event is folded against the schedule's current stored projection (read back
//! from the store), and the checkpoint advances per event so a restart resumes where it
//! left off. A per-event anomaly (undecodable, foreign subject, misrouted, an
//! invalid transition) is logged and skipped, never wedging the projector.

#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use std::collections::HashSet;

use async_nats::jetstream;
use futures::StreamExt;
use trogon_nats::jetstream::JetStreamGetStream;

use crate::error::SchedulerError;
use crate::kv::open_events_stream;
use crate::projections::schedules::storage::read_model_key;
use crate::projections::schedules::{
    ProjectionChange, ScheduleStreamState, apply, decode_recorded_delivery_message, event_message_sequence,
    event_replay_consumer_config, event_schedule_id, projection_change, read_model_token_from_event_subject,
};
use crate::projections::store::SchedulesProjectionStore;
use crate::queries::ScheduleId;
use crate::v1;

/// Projects the schedule event stream into a [`SchedulesProjectionStore`].
#[derive(Clone)]
pub struct SchedulesProjector<S> {
    store: S,
}

impl<S: SchedulesProjectionStore> SchedulesProjector<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Folds the event stream from the backend's checkpoint up to the current tail,
    /// then returns. Idempotent: re-running re-folds only what is past the
    /// checkpoint, and folding an already-applied event yields the same projection.
    ///
    /// On a from-zero rebuild (checkpoint at 0) it also reconciles: the full replay
    /// is authoritative for which schedules should exist, so any stale row left in
    /// the backend (a reused database, a reset checkpoint) is removed before the
    /// checkpoint is written.
    pub async fn catch_up<J>(&self, js: &J) -> Result<(), SchedulerError>
    where
        J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
    {
        let stream = open_events_stream(js).await?;
        let info = stream.get_info().await.map_err(|source| {
            SchedulerError::event_source("failed to query events stream info for projector catch-up", source)
        })?;
        if info.state.messages == 0 {
            return Ok(());
        }
        let target = info.state.last_sequence;
        let checkpoint = self.store.read_checkpoint().await?;
        if checkpoint >= target {
            return Ok(());
        }

        // A from-zero rebuild replays the whole log, folding each stream from
        // empty, so the store must start empty: clearing it both satisfies that and
        // drops any orphaned rows from a reused database or a reset checkpoint. A
        // resume (checkpoint > 0) only sees a suffix, so it folds onto the existing
        // rows and must not clear.
        if checkpoint == 0 {
            self.store.reconcile(&HashSet::new()).await?;
        }

        // Resume after the last folded sequence; the ordered consumer filters to
        // schedule-event subjects and replays in per-subject order.
        let consumer = stream
            .create_consumer(event_replay_consumer_config(checkpoint.saturating_add(1)))
            .await
            .map_err(|source| SchedulerError::event_source("failed to create projector catch-up consumer", source))?;
        let mut messages = consumer.messages().await.map_err(|source| {
            SchedulerError::event_source("failed to open projector catch-up message stream", source)
        })?;
        // The message stream can end before the tail is drained. Only advance the
        // checkpoint once `target` is actually folded; otherwise leave it so the
        // next start re-folds the gap instead of declaring it caught up.
        let mut reached_target = false;
        while let Some(message) = messages.next().await {
            let message = message.map_err(|source| {
                SchedulerError::event_source("failed to read schedule event during projector catch-up", source)
            })?;
            let sequence = event_message_sequence(&message, "failed to read projector catch-up event metadata")?;
            self.project_message(&message).await?;
            if sequence >= target {
                reached_target = true;
                break;
            }
        }

        if !reached_target {
            tracing::warn!(
                target,
                "projector catch-up ended before reaching the stream tail; will re-fold on next start"
            );
            return Ok(());
        }

        // Advance the checkpoint once the pass is complete; a crash mid-rebuild
        // leaves it unmoved so the next start re-folds (idempotently) from empty.
        self.store.write_checkpoint(target).await
    }

    /// Tails the stream from the backend's checkpoint indefinitely, folding each
    /// event as it arrives. Returns only on a stream error.
    pub async fn run<J>(&self, js: &J) -> Result<(), SchedulerError>
    where
        J: JetStreamGetStream<Stream = jetstream::stream::Stream>,
    {
        let stream = open_events_stream(js).await?;
        let start = self.store.read_checkpoint().await?.saturating_add(1);
        let consumer = stream
            .create_consumer(event_replay_consumer_config(start))
            .await
            .map_err(|source| SchedulerError::event_source("failed to create projector consumer", source))?;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|source| SchedulerError::event_source("failed to open projector message stream", source))?;
        while let Some(message) = messages.next().await {
            let message = message.map_err(|source| {
                SchedulerError::event_source("failed to read schedule event during projector run", source)
            })?;
            let sequence = event_message_sequence(&message, "failed to read projector event metadata")?;
            // A logged-and-skipped anomaly (foreign, malformed, misrouted, or an
            // invalid transition) produces no read-model change, so advancing past
            // it is correct and necessary for liveness — holding would re-skip the
            // same sequence forever. A real store failure returns `Err` from
            // `project_message` and short-circuits via `?` above, leaving the
            // checkpoint unadvanced so the next run re-folds that sequence.
            self.project_message(&message).await?;
            self.store.write_checkpoint(sequence).await?;
        }
        Ok(())
    }

    /// Folds one delivered event into the backing store. Per-event anomalies are
    /// logged and skipped; only a store write failure propagates.
    async fn project_message(&self, message: &jetstream::Message) -> Result<(), SchedulerError> {
        let event = match decode_recorded_delivery_message(message) {
            Ok(event) => event,
            Err(source) => {
                tracing::warn!(%source, "skipping undecodable schedule event during projection");
                return Ok(());
            }
        };
        let decoded = match event.decode::<v1::ScheduleEvent>() {
            Ok(decoded) => decoded,
            Err(source) => {
                tracing::warn!(%source, "skipping unparseable schedule event during projection");
                return Ok(());
            }
        };
        let Some(decoded) = decoded.into_decoded() else {
            // A foreign or newer-than-this-deploy event type: not part of this
            // read model, skip without disturbing state.
            return Ok(());
        };
        let subject_token = match read_model_token_from_event_subject(event.stream_id()) {
            Ok(token) => token,
            Err(source) => {
                tracing::warn!(%source, "skipping schedule event with unrecognized subject during projection");
                return Ok(());
            }
        };
        let Some(schedule_id) = event_schedule_id(&decoded) else {
            tracing::warn!(%subject_token, "skipping schedule event without a payload schedule id during projection");
            return Ok(());
        };
        if read_model_key(schedule_id) != subject_token {
            tracing::warn!(
                %subject_token,
                %schedule_id,
                "skipping schedule event whose payload id does not route to its subject during projection"
            );
            return Ok(());
        }
        // The payload id was validated when the command produced the event, so this
        // parse is defensive; an unexpected failure is skipped like any other anomaly.
        let id = match ScheduleId::parse(schedule_id) {
            Ok(id) => id,
            Err(source) => {
                tracing::warn!(%schedule_id, %source, "skipping schedule event with an invalid payload id during projection");
                return Ok(());
            }
        };

        let before = self
            .store
            .get_projection(&id)
            .await?
            .map_or(ScheduleStreamState::Initial, ScheduleStreamState::Present);
        let after = match apply(schedule_id, before.clone(), &decoded) {
            Ok(after) => after,
            Err(source) => {
                tracing::warn!(%schedule_id, %source, "skipping invalid schedule transition during projection");
                return Ok(());
            }
        };
        match projection_change(&before, &after) {
            Some(ProjectionChange::Upsert(projection)) => self.store.upsert_projection(&projection).await,
            // The change always targets this message's schedule, so reuse `id`.
            Some(ProjectionChange::Delete(_)) => self.store.delete_projection(&id).await,
            None => Ok(()),
        }
    }
}
