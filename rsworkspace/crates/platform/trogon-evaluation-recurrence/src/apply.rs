use std::convert::Infallible;

use buffa::MessageName;
use trogon_decider_nats::{CheckpointSequence, ProjectionApply, ProjectionCheckpointStore};
use trogon_decider_runtime::{Event, StreamEvent};
use trogonai_proto::evaluation::bindings::EvaluationBindingEventCase;
use trogonai_proto::evaluation::bindings::v1alpha1::{
    EvaluationBindingEvent, EvaluationResultRecorded, RubricBound, RubricUnbound,
};

use crate::projection::FindingRecurrenceProjection;

/// A stored event claimed to be part of the binding event set but could not be decoded.
#[derive(Debug, thiserror::Error)]
#[error("failed to decode an evaluation binding event: {0}")]
pub struct RecurrenceApplyError(#[source] buffa::DecodeError);

/// Applies binding stream events to a [`FindingRecurrenceProjection`] for a `Projector` catch-up.
/// Event types outside the binding event set are skipped, as they stay part of stream history.
pub struct RecurrenceApply<'a> {
    projection: &'a mut FindingRecurrenceProjection,
}

impl<'a> RecurrenceApply<'a> {
    pub fn new(projection: &'a mut FindingRecurrenceProjection) -> Self {
        Self { projection }
    }
}

impl ProjectionApply for RecurrenceApply<'_> {
    type Error = RecurrenceApplyError;

    async fn apply(&mut self, event: StreamEvent) -> Result<CheckpointSequence, Self::Error> {
        if let Some(case) = decode_case(&event.event).transpose()? {
            self.projection.fold(&EvaluationBindingEvent { event: Some(case) });
        }
        Ok(CheckpointSequence::from(event.stream_position))
    }
}

fn decode_case(event: &Event) -> Option<Result<EvaluationBindingEventCase, RecurrenceApplyError>> {
    decode::<RubricBound>(event)
        .or_else(|| decode::<RubricUnbound>(event))
        .or_else(|| decode::<EvaluationResultRecorded>(event))
}

fn decode<Payload>(event: &Event) -> Option<Result<EvaluationBindingEventCase, RecurrenceApplyError>>
where
    Payload: buffa::Message + MessageName,
    EvaluationBindingEventCase: From<Payload>,
{
    (event.r#type == Payload::FULL_NAME).then(|| {
        Payload::decode_from_slice(&event.content)
            .map(EvaluationBindingEventCase::from)
            .map_err(RecurrenceApplyError)
    })
}

/// Checkpoint store that never remembers progress, so every catch-up replays the stream from the
/// start. Recurrence is recomputed from the streams and holds no stored state (ADR 0024).
#[derive(Clone, Copy, Debug, Default)]
pub struct RebuildFromZero;

impl ProjectionCheckpointStore for RebuildFromZero {
    type Error = Infallible;

    async fn load(&self) -> Result<CheckpointSequence, Self::Error> {
        Ok(CheckpointSequence::NONE)
    }

    async fn save(&self, _checkpoint: CheckpointSequence) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
