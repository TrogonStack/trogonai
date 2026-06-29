//! Drives the Postgres schedules projection from the schedule event stream.
//!
//! This is additive and standalone: it does not touch the NATS KV projector or the
//! event store's append path. It reuses the schedules fold state machine
//! ([`crate::projections::schedules::apply`]) so the Postgres read model is
//! identical to the NATS one — event-for-event.
//!
//! A from-zero rebuild folds the whole log in memory and upserts each row in place
//! (so the table is never emptied — existing rows stay visible and are overwritten),
//! then reconciles away rows the replay did not produce. A resume folds each new
//! event onto the row already in the store. A per-event anomaly (undecodable,
//! foreign subject, misrouted, an invalid transition) is logged and skipped, never
//! wedging the projector.

#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use std::collections::{BTreeMap, HashSet};

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
use crate::queries::ScheduleId;
use crate::v1;

use super::store::PostgresSchedulesProjection;

/// Projects the schedule event stream into a [`PostgresSchedulesProjection`].
#[derive(Clone)]
pub struct SchedulesProjector {
    store: PostgresSchedulesProjection,
}

impl SchedulesProjector {
    pub fn new(store: PostgresSchedulesProjection) -> Self {
        Self { store }
    }

    /// Folds the event stream from the store's checkpoint up to the current tail,
    /// then returns. Idempotent: re-running re-folds only what is past the
    /// checkpoint, and folding an already-applied event yields the same projection.
    ///
    /// A from-zero rebuild (checkpoint at 0) folds in memory and upserts each row in
    /// place — the table is never emptied, so concurrent reads see a complete (if
    /// briefly stale) set rather than partial data — then reconciles away rows the
    /// replay did not produce. The checkpoint advances only after the pass reaches
    /// the tail, so a crash mid-rebuild leaves it untouched and the next start
    /// re-folds idempotently.
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

        // Resume after the last folded sequence; the ordered consumer filters to
        // schedule-event subjects and replays in per-subject order.
        let from_zero = checkpoint == 0;
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
        let mut rebuilt = RebuildState::default();
        let mut reached_target = false;
        while let Some(message) = messages.next().await {
            let message = message.map_err(|source| {
                SchedulerError::event_source("failed to read schedule event during projector catch-up", source)
            })?;
            let sequence = event_message_sequence(&message, "failed to read projector catch-up event metadata")?;
            if from_zero {
                self.fold_in_memory(&message, &mut rebuilt).await?;
            } else {
                self.project_message(&message).await?;
            }
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

        if from_zero {
            // The full replay is authoritative for which schedules should exist, so
            // drop any row it did not produce (a reused database, a reset checkpoint).
            self.store.reconcile(&rebuilt.live).await?;
        }
        self.store.write_checkpoint(target).await
    }

    /// Tails the stream from the store's checkpoint indefinitely, folding each event
    /// as it arrives. Returns only on a stream error.
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

    /// Folds one delivered event onto the schedule's current stored projection
    /// (resume and live paths). Per-event anomalies are logged and skipped; only a
    /// store failure propagates.
    async fn project_message(&self, message: &jetstream::Message) -> Result<(), SchedulerError> {
        let Some(routed) = route(message) else {
            return Ok(());
        };
        let before = self
            .store
            .get_projection(&routed.id)
            .await?
            .map_or(ScheduleStreamState::Initial, ScheduleStreamState::Present);
        let after = match apply(&routed.schedule_id, before.clone(), &routed.event) {
            Ok(after) => after,
            Err(source) => {
                tracing::warn!(schedule_id = %routed.schedule_id, %source, "skipping invalid schedule transition during projection");
                return Ok(());
            }
        };
        match projection_change(&before, &after) {
            Some(ProjectionChange::Upsert(projection)) => self.store.upsert_projection(&projection).await,
            // The change always targets this message's schedule, so reuse its id.
            Some(ProjectionChange::Delete(_)) => self.store.delete_projection(&routed.id).await,
            None => Ok(()),
        }
    }

    /// Folds one delivered event during a from-zero rebuild: the prior state comes
    /// from the in-memory `states` (so the replay starts from empty and never reads
    /// stale rows), each change is upserted/deleted in place, and `live` tracks the
    /// ids the replay has produced for the closing reconcile.
    async fn fold_in_memory(
        &self,
        message: &jetstream::Message,
        rebuilt: &mut RebuildState,
    ) -> Result<(), SchedulerError> {
        let Some(routed) = route(message) else {
            return Ok(());
        };
        let before = rebuilt
            .states
            .get(&routed.schedule_id)
            .cloned()
            .unwrap_or(ScheduleStreamState::Initial);
        let after = match apply(&routed.schedule_id, before.clone(), &routed.event) {
            Ok(after) => after,
            Err(source) => {
                tracing::warn!(schedule_id = %routed.schedule_id, %source, "skipping invalid schedule transition during rebuild");
                return Ok(());
            }
        };
        if let Some(change) = projection_change(&before, &after) {
            match &change {
                ProjectionChange::Upsert(projection) => {
                    self.store.upsert_projection(projection).await?;
                    rebuilt.live.insert(routed.id.clone());
                }
                ProjectionChange::Delete(_) => {
                    self.store.delete_projection(&routed.id).await?;
                    rebuilt.live.remove(&routed.id);
                }
            }
        }
        match after {
            ScheduleStreamState::Initial => {
                rebuilt.states.remove(&routed.schedule_id);
            }
            present_or_deleted => {
                rebuilt.states.insert(routed.schedule_id, present_or_deleted);
            }
        }
        Ok(())
    }
}

/// The in-memory fold of a from-zero rebuild: the per-schedule state and the set of
/// ids the replay has produced (for the closing reconcile).
#[derive(Default)]
struct RebuildState {
    states: BTreeMap<String, ScheduleStreamState>,
    live: HashSet<ScheduleId>,
}

/// One delivered event resolved to its schedule.
struct RoutedEvent {
    schedule_id: String,
    id: ScheduleId,
    event: v1::ScheduleEvent,
}

/// Decodes and routes a delivered message to its schedule, or `None` (logged) for a
/// per-event anomaly: an undecodable/unparseable payload, a foreign event type, an
/// unrecognized or mismatched subject, or an invalid id.
fn route(message: &jetstream::Message) -> Option<RoutedEvent> {
    let event = match decode_recorded_delivery_message(message) {
        Ok(event) => event,
        Err(source) => {
            tracing::warn!(%source, "skipping undecodable schedule event during projection");
            return None;
        }
    };
    let decoded = match event.decode::<v1::ScheduleEvent>() {
        Ok(decoded) => decoded,
        Err(source) => {
            tracing::warn!(%source, "skipping unparseable schedule event during projection");
            return None;
        }
    };
    // A foreign or newer-than-this-deploy event type: not part of this read model.
    let decoded = decoded.into_decoded()?;
    let subject_token = match read_model_token_from_event_subject(event.stream_id()) {
        Ok(token) => token,
        Err(source) => {
            tracing::warn!(%source, "skipping schedule event with unrecognized subject during projection");
            return None;
        }
    };
    let Some(schedule_id) = event_schedule_id(&decoded) else {
        tracing::warn!(%subject_token, "skipping schedule event without a payload schedule id during projection");
        return None;
    };
    if read_model_key(schedule_id) != subject_token {
        tracing::warn!(
            %subject_token,
            %schedule_id,
            "skipping schedule event whose payload id does not route to its subject during projection"
        );
        return None;
    }
    // The payload id was validated when the command produced the event, so this
    // parse is defensive; an unexpected failure is skipped like any other anomaly.
    let id = match ScheduleId::parse(schedule_id) {
        Ok(id) => id,
        Err(source) => {
            tracing::warn!(%schedule_id, %source, "skipping schedule event with an invalid payload id during projection");
            return None;
        }
    };
    let schedule_id = schedule_id.to_string();
    Some(RoutedEvent {
        schedule_id,
        id,
        event: decoded,
    })
}
