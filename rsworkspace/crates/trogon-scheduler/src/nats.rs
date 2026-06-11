#![cfg_attr(coverage, allow(dead_code))]

use crate::{
    config::ScheduleWriteState,
    error::SchedulerError,
    kv::{EVENTS_STREAM, EVENTS_SUBJECT_PATTERN, EVENTS_SUBJECT_PREFIX},
};
use async_nats::jetstream::{self};

pub(crate) fn event_subject(job_id: &str) -> String {
    format!("{EVENTS_SUBJECT_PREFIX}{job_id}")
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScheduleWriteCondition;
    use trogon_decider_runtime::StreamPosition;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    #[test]
    fn write_condition_rejects_unexpected_position() {
        let error = ScheduleWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", ScheduleWriteState::new(Some(position(4)), true))
            .unwrap_err();

        assert!(matches!(
            error,
            SchedulerError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn new_streams_use_canonical_event_subject() {
        let state = resolve_event_subject_state(None);

        assert_eq!(state.write_state.current_position(), None);
        assert!(!state.write_state.exists());
    }

    #[test]
    fn deleted_streams_keep_their_subject_and_still_count_as_existing() {
        let state = resolve_event_subject_state(Some(ScheduleWriteState::new(Some(position(12)), true)));

        assert_eq!(state.write_state.current_position(), Some(position(12)));
        assert!(state.write_state.exists());
    }
}
