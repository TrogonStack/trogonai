use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandStateModel, Decide, Decision, ExecuteError, ExpectedStateProvider,
    NonEmpty, Snapshot, SnapshotStateModel, SnapshotStoreConfig, StreamCommand,
    execute_command_with_snapshots,
};

use crate::{
    JobId,
    commands::{CronCommandRuntime, CronCommandRuntimePort, CronCommandSnapshotRuntime},
    error::CronError,
    events::JobEvent,
};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveJobDecisionError {
    JobNotFound { id: JobId },
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
        }
    }
}

impl std::error::Error for RemoveJobDecisionError {}

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
        }
    }
}

impl CommandStateModel for RemoveJobCommand {
    type State = RemoveJobState;
    type Event = JobEvent;
    type DomainError = CronError;

    fn initial_state() -> Self::State {
        RemoveJobState::Missing
    }

    fn evolve(_state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobRegistered { .. } | JobEvent::JobStateChanged { .. } => {
                Ok(RemoveJobState::Present)
            }
            JobEvent::JobRemoved { .. } => Ok(RemoveJobState::Missing),
        }
    }
}

impl SnapshotStateModel for RemoveJobCommand {
    type Snapshot = RemoveJobState;

    fn snapshot_state(state: &Self::State, version: u64) -> Option<Snapshot<Self::Snapshot>> {
        Some(Snapshot::new(version, *state))
    }
}

impl ExpectedStateProvider for RemoveJobCommand {}

pub async fn run<R>(runtime: &R, command: RemoveJobCommand) -> Result<(), CronError>
where
    R: CronCommandRuntimePort + CronCommandSnapshotRuntime<RemoveJobState>,
{
    let id = command.stream_id().to_string();
    let runtime = CronCommandRuntime::new(runtime, SNAPSHOT_STORE_CONFIG);

    match execute_command_with_snapshots(&runtime, &command, &AlwaysSnapshot).await {
        Ok(_) => Ok(()),
        Err(ExecuteError::Decision(RemoveJobDecisionError::JobNotFound { .. })) => {
            Err(CronError::JobNotFound { id })
        }
        Err(ExecuteError::LoadSnapshot(error))
        | Err(ExecuteError::SaveSnapshot(error))
        | Err(ExecuteError::ReadStream(error))
        | Err(ExecuteError::Append(error))
        | Err(ExecuteError::Domain(error)) => Err(error),
        Err(ExecuteError::EncodeEvent(source)) => Err(CronError::event_source(
            "failed to encode job removal event",
            source,
        )),
        Err(ExecuteError::DecodeEvent(source)) => Err(CronError::event_source(
            "failed to decode job event while catching up remove-job state",
            source,
        )),
        Err(ExecuteError::SnapshotAheadOfStream {
            snapshot_version,
            stream_version,
        }) => Err(CronError::event_source(
            "loaded remove-job snapshot is ahead of the stream state",
            std::io::Error::other(format!(
                "job '{id}' snapshot version {snapshot_version} > stream version {stream_version:?}"
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use trogon_eventsourcing::{Decision, NonEmpty, decide};

    use super::*;
    use crate::{
        DeliverySpec, GetJobCommand, JobEnabledState, JobSpec, ScheduleSpec, mocks::MockCronStore,
    };

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
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

    #[tokio::test]
    async fn run_removes_existing_job() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        run(
            &store,
            RemoveJobCommand::new(JobId::parse("backup").unwrap()),
        )
        .await
        .unwrap();

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
        assert_eq!(command_snapshot, Snapshot::new(2, RemoveJobState::Missing));
    }
}
