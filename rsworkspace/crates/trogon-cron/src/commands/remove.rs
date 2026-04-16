use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandFailure, CommandInfraError, CommandOutcome,
    CommandState, Decide, Decision, EventStore, NonEmpty, OccPolicy, SnapshotState, SnapshotStore,
    SnapshotStoreConfig, Snapshots, StreamCommand,
};

use crate::{JobId, error::CronError, events::JobEvent};

#[derive(Debug, Clone)]
pub struct RemoveJobCommand {
    pub id: JobId,
}

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.remove_job.v1.", None);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RemoveJobState {
    Missing,
    Present,
    Deleted,
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

impl std::error::Error for RemoveJobDecisionError {}

pub type RemoveJobResult = Result<
    CommandOutcome<JobEvent>,
    CommandFailure<RemoveJobDecisionError, CommandInfraError<CronError>>,
>;

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
            RemoveJobState::Present => Ok(Decision::Event(NonEmpty::one(JobEvent::JobRemoved {
                id: command.stream_id().to_string(),
            }))),
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
            JobEvent::JobRegistered { .. } | JobEvent::JobStateChanged { .. } => match state {
                RemoveJobState::Deleted => Ok(RemoveJobState::Deleted),
                RemoveJobState::Missing | RemoveJobState::Present => Ok(RemoveJobState::Present),
            },
            JobEvent::JobRemoved { .. } => Ok(RemoveJobState::Deleted),
        }
    }
}

impl SnapshotState for RemoveJobCommand {
    type Snapshot = RemoveJobState;

    fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot> {
        Some(*state)
    }
}

pub async fn run<E, S>(
    event_store: &E,
    snapshot_store: &S,
    command: RemoveJobCommand,
    occ: OccPolicy,
) -> RemoveJobResult
where
    E: EventStore<JobId, Error = CronError>,
    S: SnapshotStore<RemoveJobState, JobId, Error = CronError>,
{
    Ok(CommandExecution::new(event_store, &command)
        .occ(occ)
        .snapshots(Snapshots::new(
            snapshot_store,
            SNAPSHOT_STORE_CONFIG,
            AlwaysSnapshot,
        ))
        .execute()
        .await?
        .into_outcome())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use trogon_eventsourcing::{
        Decision, NonEmpty, Snapshot, decide,
        testing::{TestCase, decider, expect_error},
    };

    use super::*;
    use crate::{
        DeliverySpec, GetJobCommand, JobEnabledState, JobSpec, ScheduleSpec, mocks::MockCronStore,
    };

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: job_id(id),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn decides_removal_from_present_state() {
        let state = RemoveJobState::Present;
        let command = RemoveJobCommand::new(JobId::parse("backup").unwrap());

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobRemoved {
                id: "backup".to_string(),
            }))
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
            .given([JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: crate::RegisteredJobSpec::from(job("backup")),
            }])
            .when(RemoveJobCommand::new(JobId::parse("backup").unwrap()))
            .then([JobEvent::JobRemoved {
                id: "backup".to_string(),
            }]);
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

        let outcome = run(
            &store,
            &store,
            RemoveJobCommand::new(JobId::parse("backup").unwrap()),
            OccPolicy::UseCommandRule,
        )
        .await
        .unwrap();
        assert_eq!(outcome.next_expected_version, 2);
        assert_eq!(
            outcome.events,
            NonEmpty::one(JobEvent::JobRemoved {
                id: "backup".to_string(),
            })
        );

        assert!(
            store
                .get_job(GetJobCommand {
                    id: JobId::parse("backup").unwrap(),
                })
                .await
                .unwrap()
                .is_none()
        );

        let command_snapshot = store
            .read_command_snapshot::<RemoveJobState>(
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(command_snapshot, Snapshot::new(2, RemoveJobState::Deleted));
    }
}
