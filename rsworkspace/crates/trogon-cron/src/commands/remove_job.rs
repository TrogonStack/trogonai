use std::convert::Infallible;

use trogon_eventsourcing::{
    CommandExecution, CommandResult, CommandSnapshots, Decide, Decision, FrequencySnapshot, NonEmpty, OccPolicy,
    SnapshotRead, SnapshotWrite, StreamAppend, StreamCommand, StreamRead,
};

use super::JobCommandState;
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

impl Decide for RemoveJobCommand {
    type State = JobCommandState;
    type Event = JobEvent;
    type EvolveError = Infallible;
    type DecideError = RemoveJobDecisionError;

    fn initial_state() -> JobCommandState {
        JobCommandState::initial_state()
    }

    fn evolve(state: JobCommandState, event: JobEvent) -> Result<JobCommandState, Self::EvolveError> {
        state.evolve(event)
    }

    fn decide(state: &JobCommandState, command: &Self) -> Result<Decision<JobEvent>, Self::DecideError> {
        match state {
            JobCommandState::Missing => Err(RemoveJobDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            JobCommandState::Present { .. } => Ok(Decision::Event(NonEmpty::one(JobEvent::JobRemoved(JobRemoved {
                id: command.stream_id().to_string(),
            })))),
            JobCommandState::Deleted => Err(RemoveJobDecisionError::JobDeleted {
                id: command.stream_id().clone(),
            }),
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
) -> CommandResult<RemoveJobCommand, SErr>
where
    S: StreamRead<JobId, Error = SErr>
        + StreamAppend<JobId, Error = SErr>
        + SnapshotRead<JobCommandState, JobId, Error = SErr>
        + SnapshotWrite<JobCommandState, JobId, Error = SErr>,
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
        let state = JobCommandState::Present {
            current: JobEnabledState::Enabled,
        };
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
        let state = JobCommandState::Missing;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RemoveJobDecisionError::JobNotFound { .. }
        ));
    }

    #[test]
    fn rejects_removing_deleted_job() {
        let state = JobCommandState::Deleted;
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
            .read_command_snapshot::<JobCommandState>(
                JobCommandState::snapshot_store_config(),
                &JobId::parse("backup").unwrap(),
            )
            .unwrap();
        assert!(command_snapshot.is_none());
    }
}
