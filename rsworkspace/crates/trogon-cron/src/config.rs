use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{StreamPosition, StreamState};

use crate::error::CronError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", content = "position", rename_all = "snake_case")]
pub enum JobWriteCondition {
    MustNotExist,
    MustBeAtPosition(StreamPosition),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobWriteState {
    current_position: Option<StreamPosition>,
    exists: bool,
}

impl JobWriteState {
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

impl JobWriteCondition {
    pub fn ensure(self, id: &str, state: JobWriteState) -> Result<(), CronError> {
        match self {
            Self::MustNotExist if !state.exists() => Ok(()),
            Self::MustBeAtPosition(expected) if state.current_position() == Some(expected) => Ok(()),
            expected => Err(CronError::OptimisticConcurrencyConflict {
                id: id.to_string(),
                expected: expected.into(),
                current_position: state.current_position(),
            }),
        }
    }
}

impl From<JobWriteCondition> for StreamState {
    fn from(value: JobWriteCondition) -> Self {
        match value {
            JobWriteCondition::MustNotExist => Self::NoStream,
            JobWriteCondition::MustBeAtPosition(position) => Self::At(position),
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
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(None, false))
            .unwrap();
        JobWriteCondition::MustBeAtPosition(position(3))
            .ensure("alpha", JobWriteState::new(Some(position(3)), true))
            .unwrap();

        let error = JobWriteCondition::MustBeAtPosition(position(2))
            .ensure("alpha", JobWriteState::new(Some(position(4)), true))
            .unwrap_err();
        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn write_condition_rejects_reusing_deleted_stream_ids() {
        let error = JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(Some(position(7)), true))
            .unwrap_err();

        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_position: Some(_),
                ..
            }
        ));
    }
}
