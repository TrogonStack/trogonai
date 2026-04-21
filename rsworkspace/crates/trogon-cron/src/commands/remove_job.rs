use serde::{Deserialize, Serialize};
use trogon_eventsourcing::snapshot::SnapshotSchema;
use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshots, CommandState, Decide, Decision,
    FrequencySnapshot, NonEmpty, OccPolicy, SnapshotRead, SnapshotWrite, StreamAppend,
    StreamCommand, StreamRead,
};

use crate::{
    JobId,
    events::{JobAdded, JobEvent, JobPaused, JobRemoved, JobResumed},
};

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoveJobState {
    Missing,
    Present,
    Deleted,
}

impl SnapshotSchema for RemoveJobState {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.remove_job.v1.";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveJobDecisionError {
    JobNotFound { id: JobId },
    JobDeleted { id: JobId },
}

impl RemoveJobCommand {
    pub const fn new(id: JobId) -> Self {
        Self { id }
    }
}

impl std::fmt::Display for RemoveJobDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for removal '{id}'"),
            Self::JobDeleted { id } => {
                write!(
                    f,
                    "job '{id}' was already deleted and cannot be removed again"
                )
            }
        }
    }
}

impl StreamCommand for RemoveJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<RemoveJobState, JobEvent> for RemoveJobCommand {
    type Error = RemoveJobDecisionError;

    fn decide(state: &RemoveJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            RemoveJobState::Missing => Err(RemoveJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            RemoveJobState::Present => Ok(Decision::Event(NonEmpty::one(JobEvent::JobRemoved(
                JobRemoved {
                    id: command.stream_id().to_string(),
                },
            )))),
            RemoveJobState::Deleted => Err(RemoveJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandState for RemoveJobCommand {
    type State = RemoveJobState;
    type Event = JobEvent;
    type DomainError = RemoveJobDecisionError;

    fn initial_state() -> Self::State {
        RemoveJobState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobAdded(JobAdded { .. })
            | JobEvent::JobPaused(JobPaused { .. })
            | JobEvent::JobResumed(JobResumed { .. }) => match state {
                RemoveJobState::Deleted => Ok(RemoveJobState::Deleted),
                RemoveJobState::Missing | RemoveJobState::Present => Ok(RemoveJobState::Present),
            },
            JobEvent::JobRemoved(JobRemoved { .. }) => Ok(RemoveJobState::Deleted),
        }
    }
}

impl CommandSnapshots for RemoveJobCommand {
    type SnapshotPolicy = FrequencySnapshot;

    fn snapshot_policy() -> Self::SnapshotPolicy {
        super::command_snapshot_policy()
    }
}

pub async fn remove_job<S, SErr>(
    store: &S,
    command: RemoveJobCommand,
    occ: Option<OccPolicy>,
) -> CommandResult<RemoveJobState, JobEvent, RemoveJobDecisionError, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<RemoveJobState, JobId, Error = SErr>
        + SnapshotWrite<RemoveJobState, JobId, Error = SErr>,
    serde_json::Error: Into<SErr>,
{
    CommandExecution::new(store, &command)
        .with_occ(occ)
        .with_snapshot(store)
        .execute()
        .await
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{
        Decision, NonEmpty, decide,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{
        DeliverySpec, GetJobCommand, JobEnabledState, JobSpec, MessageContent, MessageHeaders,
        ScheduleSpec, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::nats_event("agent.run").unwrap(),
            content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
            headers: MessageHeaders::default(),
        }
    }

    #[test]
    fn decides_removal_from_present_state() {
        let state = RemoveJobState::Present;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobRemoved(JobRemoved {
                id: "backup".to_string(),
            })))
        );
    }

    #[test]
    fn rejects_removing_missing_job() {
        let state = RemoveJobState::Missing;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RemoveJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_removing_deleted_job() {
        let state = RemoveJobState::Deleted;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RemoveJobDecisionError::JobDeleted { .. }
        ));
    }

    #[test]
    fn given_when_then_supports_remove_job_decider() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([JobEvent::JobAdded(JobAdded {
                id: "backup".to_string(),
                job: crate::JobDetails::from(job("backup")),
            })])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobRemoved(JobRemoved {
                id: "backup".to_string(),
            })]);
    }

    #[test]
    fn given_when_then_supports_remove_job_failures() {
        TestCase::new(decider::<RemoveJobCommand>())
            .given([])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then(expect_error(RemoveJobDecisionError::JobNotFound {
                id: JobId::parse("backup").unwrap(),
            }));
    }

    #[tokio::test]
    async fn run_removes_existing_job() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        let outcome = remove_job(
            &store,
            RemoveJobCommand::new(JobId::parse("backup").unwrap()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobRemoved(JobRemoved {
                id: "backup".to_string(),
            }))
        );

        assert!(
            store
                .get_job(GetJobCommand {
                    id: "backup".to_string(),
                })
                .await
                .unwrap()
                .is_none()
        );

        let command_snapshot = store
            .read_command_snapshot::<RemoveJobState>(
                RemoveJobState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
