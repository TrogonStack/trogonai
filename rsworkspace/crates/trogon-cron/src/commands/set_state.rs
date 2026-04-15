use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandStateModel, Decide, Decision,
    DefaultExpectedStateProvider, ExecuteError, NonEmpty, OccPolicy, SnapshotStateModel,
    SnapshotStoreConfig, StreamCommand,
};

use crate::{
    JobEnabledState, JobId,
    commands::{CronCommandRuntime, CronCommandRuntimePort, CronCommandSnapshotRuntime},
    error::CronError,
    events::JobEvent,
};

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.change_job_state.v1.", None);

#[derive(Debug, Clone)]
pub struct ChangeJobStateCommand {
    pub id: JobId,
    pub state: JobEnabledState,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChangeJobStateState {
    Missing,
    Present { current: JobEnabledState },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeJobStateDecisionError {
    JobNotFound { id: JobId },
    StateAlreadySet { id: JobId, state: JobEnabledState },
}

impl ChangeJobStateCommand {
    pub const fn new(id: JobId, state: JobEnabledState) -> Self {
        Self { id, state }
    }
}

impl std::fmt::Display for ChangeJobStateDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JobNotFound { id } => write!(f, "missing job for state change '{id}'"),
            Self::StateAlreadySet { id, state } => {
                write!(f, "job '{id}' is already {}", state.as_str())
            }
        }
    }
}

impl std::error::Error for ChangeJobStateDecisionError {}

impl StreamCommand for ChangeJobStateCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<ChangeJobStateState, JobEvent> for ChangeJobStateCommand {
    type Error = ChangeJobStateDecisionError;

    fn decide(
        state: &ChangeJobStateState,
        command: &Self,
    ) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            ChangeJobStateState::Missing => Err(ChangeJobStateDecisionError::JobNotFound {
                id: command.stream_id().clone(),
            }),
            ChangeJobStateState::Present { current } if *current == command.state => {
                Err(ChangeJobStateDecisionError::StateAlreadySet {
                    id: command.stream_id().clone(),
                    state: command.state,
                })
            }
            ChangeJobStateState::Present { .. } => {
                Ok(Decision::Event(NonEmpty::one(JobEvent::JobStateChanged {
                    id: command.stream_id().to_string(),
                    state: command.state,
                })))
            }
        }
    }
}

impl CommandStateModel for ChangeJobStateCommand {
    type State = ChangeJobStateState;
    type Event = JobEvent;
    type DomainError = CronError;

    fn initial_state() -> Self::State {
        ChangeJobStateState::Missing
    }

    fn evolve(_state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobRegistered { id, spec } => Ok(ChangeJobStateState::Present {
                current: spec.into_job_spec(id).state,
            }),
            JobEvent::JobStateChanged { state, .. } => {
                Ok(ChangeJobStateState::Present { current: state })
            }
            JobEvent::JobRemoved { .. } => Ok(ChangeJobStateState::Missing),
        }
    }
}

impl SnapshotStateModel for ChangeJobStateCommand {
    type Snapshot = ChangeJobStateState;

    fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot> {
        Some(*state)
    }
}

impl DefaultExpectedStateProvider for ChangeJobStateCommand {}

pub async fn run<R>(runtime: &R, command: ChangeJobStateCommand) -> Result<(), CronError>
where
    R: CronCommandRuntimePort + CronCommandSnapshotRuntime<ChangeJobStateState>,
{
    run_with_occ(runtime, command, OccPolicy::CommandDefault).await
}

pub async fn run_with_occ<R>(
    runtime: &R,
    command: ChangeJobStateCommand,
    occ: OccPolicy,
) -> Result<(), CronError>
where
    R: CronCommandRuntimePort + CronCommandSnapshotRuntime<ChangeJobStateState>,
{
    let id = command.stream_id().to_string();
    let runtime = CronCommandRuntime::new(runtime, SNAPSHOT_STORE_CONFIG);

    match CommandExecution::new(&runtime, &command)
        .occ(occ)
        .snapshots(AlwaysSnapshot)
        .execute()
        .await
    {
        Ok(_) => Ok(()),
        Err(ExecuteError::Decision(ChangeJobStateDecisionError::JobNotFound { .. })) => {
            Err(CronError::JobNotFound { id })
        }
        Err(ExecuteError::Decision(ChangeJobStateDecisionError::StateAlreadySet {
            state, ..
        })) => Err(CronError::JobStateAlreadySet { id, state }),
        Err(ExecuteError::LoadSnapshot(error))
        | Err(ExecuteError::SaveSnapshot(error))
        | Err(ExecuteError::ReadStream(error))
        | Err(ExecuteError::Append(error))
        | Err(ExecuteError::Domain(error)) => Err(error),
        Err(ExecuteError::EncodeEvent(source)) => Err(CronError::event_source(
            "failed to encode job state change event",
            source,
        )),
        Err(ExecuteError::DecodeEvent(source)) => Err(CronError::event_source(
            "failed to decode job event while catching up change-job-state state",
            source,
        )),
        Err(ExecuteError::SnapshotAheadOfStream {
            snapshot_version,
            stream_version,
        }) => Err(CronError::event_source(
            "loaded change-job-state snapshot is ahead of the stream state",
            std::io::Error::other(format!(
                "job '{id}' snapshot version {snapshot_version} > stream version {stream_version:?}"
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use trogon_eventsourcing::{Decision, NonEmpty, Snapshot, decide};

    use super::*;
    use crate::{DeliverySpec, GetJobCommand, JobSpec, ScheduleSpec, mocks::MockCronStore};

    fn job(id: &str) -> JobSpec {
        JobSpec {
            id: id.to_string(),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: std::collections::BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn decides_state_change_from_present_state() {
        let state = ChangeJobStateState::Present {
            current: JobEnabledState::Enabled,
        };
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Disabled);

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobStateChanged {
                id: "backup".to_string(),
                state: JobEnabledState::Disabled,
            }))
        );
    }

    #[test]
    fn rejects_noop_state_changes() {
        let state = ChangeJobStateState::Present {
            current: JobEnabledState::Enabled,
        };
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Enabled);

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::StateAlreadySet { .. }
        ));
    }

    #[test]
    fn rejects_state_changes_for_missing_jobs() {
        let state = ChangeJobStateState::Missing;
        let command =
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Enabled);

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            ChangeJobStateDecisionError::JobNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn run_updates_job_state() {
        let store = MockCronStore::new();
        store.seed_job(job("backup"));

        run(
            &store,
            ChangeJobStateCommand::new(JobId::parse("backup").unwrap(), JobEnabledState::Disabled),
        )
        .await
        .unwrap();

        let snapshot = store
            .get_job(GetJobCommand {
                id: JobId::parse("backup").unwrap(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.payload.state, JobEnabledState::Disabled);

        let command_snapshot = store
            .read_command_snapshot::<ChangeJobStateState>(
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            command_snapshot,
            Snapshot::new(
                2,
                ChangeJobStateState::Present {
                    current: JobEnabledState::Disabled,
                },
            )
        );
    }
}
