#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use crate::{
    commands::domain::ScheduleId,
    config::ScheduleWriteState,
    constants::{EVENTS_STREAM, EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX},
    error::SchedulerError,
};
use async_nats::jetstream::{self, stream::RetentionPolicy};
use std::time::Duration;

pub(crate) fn event_subject(schedule_id: &ScheduleId) -> String {
    format!("{EVENTS_SUBJECT_PREFIX}{schedule_id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamSubjectState {
    pub(crate) write_state: ScheduleWriteState,
}

pub(crate) fn resolve_event_subject_state(canonical_state: Option<ScheduleWriteState>) -> StreamSubjectState {
    match canonical_state {
        Some(write_state) => StreamSubjectState { write_state },
        None => StreamSubjectState {
            write_state: ScheduleWriteState::new(None, false),
        },
    }
}

#[cfg(not(coverage))]
pub(crate) fn validate_events_stream(stream: &jetstream::stream::Stream) -> Result<(), SchedulerError> {
    let config = &stream.cached_info().config;
    if !config.allow_atomic_publish {
        return Err(SchedulerError::event_source(
            "events stream is missing allow_atomic",
            std::io::Error::other(EVENTS_STREAM),
        ));
    }
    if !config.subjects.iter().any(|subject| subject == EVENTS_SUBJECT_PATTERN) {
        return Err(SchedulerError::event_source(
            "events stream is missing canonical schedule event subject coverage",
            std::io::Error::other(EVENTS_STREAM),
        ));
    }
    // The read model is rebuilt by folding the full event history, so the events
    // stream must never drop a message. Refuse to start against any retention that
    // could evict events, rather than silently producing a corrupt read model.
    if config.retention != RetentionPolicy::Limits
        || config.max_age != Duration::ZERO
        || config.max_messages >= 0
        || config.max_messages_per_subject >= 0
        || config.max_bytes >= 0
    {
        return Err(SchedulerError::event_source(
            "events stream retention may drop events; it must retain full history \
             (retention=Limits, max_age=0, unlimited max_messages/max_messages_per_subject/max_bytes)",
            std::io::Error::other(EVENTS_STREAM),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
