//! Drives an alternative projection backend (a [`SchedulesProjectionStore`], e.g.
//! Postgres) from the schedule event stream.
//!
//! This is additive and standalone: it does not touch the NATS KV projection or
//! the event store's append path. It reuses this module's fold state machine
//! ([`super::apply`]) so the alternative backend produces the same read model the
//! NATS projection does — event-for-event.
//!
//! Each event is folded against the schedule's current stored view (read back from
//! the backend), and the checkpoint advances per event so a restart resumes where
//! it left off. A per-event anomaly (undecodable, foreign subject, misrouted, an
//! invalid transition) is logged and skipped, never wedging the projector.

#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use std::sync::Arc;

use async_nats::jetstream;
use futures::StreamExt;
use trogon_nats::jetstream::JetStreamGetStream;

use crate::error::SchedulerError;
use crate::kv::open_events_stream;
use crate::projections::backend::SchedulesProjectionStore;
use crate::v1;

use super::storage::read_model_key;
use super::{
    ProjectionChange, ScheduleStreamState, apply, decode_recorded_delivery_message, event_message_sequence,
    event_replay_consumer_config, event_schedule_id, projection_change, read_model_token_from_event_subject,
};

/// Projects the schedule event stream into a [`SchedulesProjectionStore`].
#[derive(Clone)]
pub struct SchedulesProjector {
    store: Arc<dyn SchedulesProjectionStore>,
}

impl SchedulesProjector {
    pub fn new(store: Arc<dyn SchedulesProjectionStore>) -> Self {
        Self { store }
    }

    /// Folds the event stream from the backend's checkpoint up to the current tail,
    /// then returns. Idempotent: re-running re-folds only what is past the
    /// checkpoint, and folding an already-applied event yields the same view.
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
        if self.store.read_checkpoint().await? >= target {
            return Ok(());
        }

        // Resume after the last folded sequence; the ordered consumer filters to
        // schedule-event subjects and replays in per-subject order.
        let start = self.store.read_checkpoint().await?.saturating_add(1);
        let consumer = stream
            .create_consumer(event_replay_consumer_config(start))
            .await
            .map_err(|source| SchedulerError::event_source("failed to create projector catch-up consumer", source))?;
        let mut messages = consumer.messages().await.map_err(|source| {
            SchedulerError::event_source("failed to open projector catch-up message stream", source)
        })?;
        while let Some(message) = messages.next().await {
            let message = message.map_err(|source| {
                SchedulerError::event_source("failed to read schedule event during projector catch-up", source)
            })?;
            let sequence = event_message_sequence(&message, "failed to read projector catch-up event metadata")?;
            self.project_message(&message).await?;
            self.store.write_checkpoint(sequence).await?;
            if sequence >= target {
                break;
            }
        }
        Ok(())
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

        let before = self
            .store
            .get_view(schedule_id)
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
            Some(ProjectionChange::Upsert(view)) => self.store.upsert_view(&view).await,
            Some(ProjectionChange::Delete(id)) => self.store.delete_view(&id).await,
            None => Ok(()),
        }
    }
}
