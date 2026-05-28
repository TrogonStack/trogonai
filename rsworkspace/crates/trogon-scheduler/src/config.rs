use serde::{Deserialize, Serialize};
use trogon_decider_runtime::{StreamPosition, StreamWritePrecondition};

use crate::error::SchedulerError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", content = "position", rename_all = "snake_case")]
pub enum ScheduleWriteCondition {
    MustNotExist,
    MustBeAtPosition(StreamPosition),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleWriteState {
    current_position: Option<StreamPosition>,
    exists: bool,
}

impl ScheduleWriteState {
    pub const fn new(current_position: Option<StreamPosition>, exists: bool) -> Self {
        Self {
            current_position,
            exists,
        }
    }

    pub const fn current_position(self) -> Option<StreamPosition> {
        self.current_position
    }

    pub const fn exists(self) -> bool {
        self.exists
    }
}

impl ScheduleWriteCondition {
    pub fn ensure(self, id: &str, state: ScheduleWriteState) -> Result<(), SchedulerError> {
        match self {
            Self::MustNotExist if !state.exists() => Ok(()),
            Self::MustBeAtPosition(expected) if state.current_position() == Some(expected) => Ok(()),
            expected => Err(SchedulerError::OptimisticConcurrencyConflict {
                id: id.to_string(),
                expected: expected.into(),
                current_position: state.current_position(),
            }),
        }
    }
}

impl From<ScheduleWriteCondition> for StreamWritePrecondition {
    fn from(value: ScheduleWriteCondition) -> Self {
        match value {
            ScheduleWriteCondition::MustNotExist => Self::NoStream,
            ScheduleWriteCondition::MustBeAtPosition(position) => Self::At(position),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    #[test]
    fn write_condition_ensures_expected_positions() {
        ScheduleWriteCondition::MustNotExist
            .ensure("alpha", ScheduleWriteState::new(None, false))
            .unwrap();
        ScheduleWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", ScheduleWriteState::new(Some(position(3)), true))
            .unwrap();

        let error = ScheduleWriteCondition::MustBeAtPosition(position(2))
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
    fn write_condition_rejects_reusing_deleted_stream_ids() {
        let error = ScheduleWriteCondition::MustNotExist
            .ensure("alpha", ScheduleWriteState::new(Some(position(7)), true))
            .unwrap_err();

        assert!(matches!(
            error,
            SchedulerError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));
    }
}
