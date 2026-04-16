use serde::{Deserialize, Serialize};
use trogon_eventsourcing::ExpectedState;

use crate::error::CronError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", content = "version", rename_all = "snake_case")]
pub enum JobWriteCondition {
    MustNotExist,
    MustBeAtVersion(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobWriteState {
    current_version: Option<u64>,
    exists: bool,
}

impl JobWriteState {
    pub const fn new(current_version: Option<u64>, exists: bool) -> Self {
        Self {
            current_version,
            exists,
        }
    }

    pub const fn current_version(self) -> Option<u64> {
        self.current_version
    }

    pub const fn exists(self) -> bool {
        self.exists
    }
}

impl JobWriteCondition {
    pub fn ensure(self, id: &str, state: JobWriteState) -> Result<(), CronError> {
        match self {
            Self::MustNotExist if !state.exists() => Ok(()),
            Self::MustBeAtVersion(expected) if state.current_version() == Some(expected) => Ok(()),
            expected => Err(CronError::OptimisticConcurrencyConflict {
                id: id.to_string(),
                expected: expected.into(),
                current_version: state.current_version(),
            }),
        }
    }
}

impl From<JobWriteCondition> for ExpectedState {
    fn from(value: JobWriteCondition) -> Self {
        match value {
            JobWriteCondition::MustNotExist => Self::NoStream,
            JobWriteCondition::MustBeAtVersion(version) => Self::StreamRevision(version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_condition_ensures_expected_versions() {
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(None, false))
            .unwrap();
        JobWriteCondition::MustBeAtVersion(3)
            .ensure("alpha", JobWriteState::new(Some(3), true))
            .unwrap();

        let error = JobWriteCondition::MustBeAtVersion(2)
            .ensure("alpha", JobWriteState::new(Some(4), true))
            .unwrap_err();
        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_version: Some(4),
                ..
            }
        ));
    }

    #[test]
    fn write_condition_rejects_reusing_deleted_stream_ids() {
        let error = JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(Some(7), true))
            .unwrap_err();

        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_version: Some(7),
                ..
            }
        ));
    }
}
