use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshotPolicy, Decide, Decision, FrequencySnapshot,
    OverrideWritePrecondition, SnapshotRead, SnapshotWrite, StreamAppend, StreamCommand, StreamRead, StreamState,
};

use super::JobState;
use crate::{
    JobId,
    events::{JobEvent, JobRemoved},
};

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
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

impl StreamCommand for RemoveJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl OverrideWritePrecondition for RemoveJobCommand {}

impl Decide for RemoveJobCommand {
    type State = JobState;
    type Event = JobEvent;
    type DecideError = RemoveJobDecisionError;

    fn decide(state: &JobState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobState::Missing => Err(RemoveJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            JobState::PresentEnabled | JobState::PresentDisabled => Ok(Decision::event(JobRemoved {
                id: command.stream_id().to_string(),
            })),
            JobState::Deleted => Err(RemoveJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandSnapshotPolicy for RemoveJobCommand {
    type SnapshotPolicy = FrequencySnapshot;
    const SNAPSHOT_POLICY: Self::SnapshotPolicy = super::snapshot::COMMAND_SNAPSHOT_POLICY;
}

pub async fn remove_job<S, SErr>(
    store: &S,
    command: RemoveJobCommand,
    write_precondition: Option<StreamState>,
) -> CommandResult<RemoveJobCommand, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<JobState, JobId, Error = SErr>
        + SnapshotWrite<JobState, JobId, Error = SErr>,
    serde_json::Error: Into<SErr>,
{
    CommandExecution::new(store, &command)
        .with_write_precondition(write_precondition)
        .with_snapshot(store)
        .execute()
        .await
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::snapshot::SnapshotSchema;
    use trogon_eventsourcing::{
        Decision, NonEmpty, decide,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{
        DeliverySpec, GetJobCommand, JobAdded, JobEnabledState, JobHeaders, JobMessage, JobSpec, MessageContent,
        ScheduleSpec, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::every(30).unwrap(),
            delivery: DeliverySpec::nats_event("agent.run").unwrap(),
            message: JobMessage {
                content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
                headers: JobHeaders::default(),
            },
        }
    }

    #[test]
    fn decides_removal_from_present_state() {
        let state = JobState::PresentEnabled;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::event(JobRemoved {
                id: "backup".to_string(),
            })
        );
    }

    #[test]
    fn rejects_removing_missing_job() {
        let state = JobState::Missing;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RemoveJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_removing_deleted_job() {
        let state = JobState::Deleted;
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

        let outcome = remove_job(&store, RemoveJobCommand::new(JobId::parse("backup").unwrap()), None)
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
                .get_job(GetJobCommand::new(JobId::parse("backup").unwrap()))
                .await
                .unwrap()
                .is_none()
        );

        let command_snapshot = store
            .read_command_snapshot::<JobState>(JobState::snapshot_store_config(), &JobId::parse("backup").unwrap())
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
