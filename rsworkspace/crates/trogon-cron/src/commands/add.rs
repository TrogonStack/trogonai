use std::fmt;

use serde::{Deserialize, Serialize};
use trogon_eventsourcing::{
    AlwaysSnapshot, CommandExecution, CommandStateModel, Decide, Decision,
    DefaultExpectedStateProvider, ExecuteError, ExecutionRuntime, ExpectedState, NonEmpty,
    OccPolicy, SnapshotStateModel, SnapshotStoreConfig, StreamCommand,
};

use crate::{
    JobId, JobSpec, ResolvedJobSpec,
    commands::{CronCommandRuntime, CronCommandRuntimePort, CronCommandSnapshotRuntime},
    error::CronError,
    events::{JobEvent, RegisteredJobSpec},
};

#[derive(Debug, Clone)]
pub struct RegisterJobCommand {
    id: JobId,
    job: ResolvedJobSpec,
}

pub(crate) const SNAPSHOT_STORE_CONFIG: SnapshotStoreConfig<'static> =
    SnapshotStoreConfig::new("cron.command.register_job.v1.", None);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegisterJobState {
    Missing,
    Present,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterJobDecisionError {
    AlreadyRegistered { id: JobId },
}

impl fmt::Display for RegisterJobDecisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered { id } => write!(f, "job '{id}' is already registered"),
        }
    }
}

impl std::error::Error for RegisterJobDecisionError {}

impl RegisterJobCommand {
    pub fn new(spec: JobSpec) -> Result<Self, CronError> {
        let id = JobId::parse(&spec.id).map_err(|source| {
            CronError::event_source(
                "failed to build register job command from validated spec",
                source,
            )
        })?;
        Ok(Self {
            id,
            job: ResolvedJobSpec::try_from(&spec)?,
        })
    }

    pub fn job(&self) -> &ResolvedJobSpec {
        &self.job
    }
}

impl StreamCommand for RegisterJobCommand {
    type StreamId = JobId;

    fn stream_id(&self) -> &Self::StreamId {
        &self.id
    }
}

impl Decide<RegisterJobState, JobEvent> for RegisterJobCommand {
    type Error = RegisterJobDecisionError;

    fn decide(state: &RegisterJobState, command: &Self) -> Result<Decision<JobEvent>, Self::Error> {
        match state {
            RegisterJobState::Missing => {
                Ok(Decision::Event(NonEmpty::one(JobEvent::JobRegistered {
                    id: command.stream_id().to_string(),
                    spec: RegisteredJobSpec::from(command.job().spec()),
                })))
            }
            RegisterJobState::Present => Err(RegisterJobDecisionError::AlreadyRegistered {
                id: command.stream_id().clone(),
            }),
        }
    }
}

impl CommandStateModel for RegisterJobCommand {
    type State = RegisterJobState;
    type Event = JobEvent;
    type DomainError = CronError;

    fn initial_state() -> Self::State {
        RegisterJobState::Missing
    }

    fn evolve(state: Self::State, event: JobEvent) -> Result<Self::State, Self::DomainError> {
        match event {
            JobEvent::JobRegistered { .. } | JobEvent::JobStateChanged { .. } => {
                Ok(RegisterJobState::Present)
            }
            JobEvent::JobRemoved { .. } => match state {
                RegisterJobState::Missing => Ok(RegisterJobState::Missing),
                RegisterJobState::Present => Ok(RegisterJobState::Missing),
            },
        }
    }
}

impl SnapshotStateModel for RegisterJobCommand {
    type Snapshot = RegisterJobState;

    fn snapshot_state(state: &Self::State) -> Option<Self::Snapshot> {
        Some(*state)
    }
}

impl DefaultExpectedStateProvider for RegisterJobCommand {
    fn default_expected_state(&self) -> Option<ExpectedState> {
        Some(ExpectedState::NoStream)
    }
}

pub async fn run<R>(runtime: &R, command: RegisterJobCommand) -> Result<(), CronError>
where
    R: CronCommandRuntimePort + CronCommandSnapshotRuntime<RegisterJobState>,
{
    run_with_occ(runtime, command, OccPolicy::CommandDefault).await
}

pub async fn run_with_occ<R>(
    runtime: &R,
    command: RegisterJobCommand,
    occ: OccPolicy,
) -> Result<(), CronError>
where
    R: CronCommandRuntimePort + CronCommandSnapshotRuntime<RegisterJobState>,
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
        Err(ExecuteError::Decision(RegisterJobDecisionError::AlreadyRegistered { .. })) => {
            let current_version = runtime.current_stream_version(command.stream_id()).await?;
            Err(CronError::OptimisticConcurrencyConflict {
                id: id.clone(),
                expected: occ.resolve(current_version, command.default_expected_state()),
                current_version,
            })
        }
        Err(ExecuteError::LoadSnapshot(error))
        | Err(ExecuteError::SaveSnapshot(error))
        | Err(ExecuteError::ReadStream(error))
        | Err(ExecuteError::Append(error))
        | Err(ExecuteError::Domain(error)) => Err(error),
        Err(ExecuteError::EncodeEvent(source)) => Err(CronError::event_source(
            "failed to encode job registration event",
            source,
        )),
        Err(ExecuteError::DecodeEvent(source)) => Err(CronError::event_source(
            "failed to decode job event while catching up register-job state",
            source,
        )),
        Err(ExecuteError::SnapshotAheadOfStream {
            snapshot_version,
            stream_version,
        }) => Err(CronError::event_source(
            "loaded register-job snapshot is ahead of the stream state",
            std::io::Error::other(format!(
                "job '{id}' snapshot version {snapshot_version} > stream version {stream_version:?}"
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use trogon_eventsourcing::{Decision, NonEmpty, Snapshot, decide};

    use super::*;
    use crate::{DeliverySpec, GetJobCommand, JobEnabledState, ScheduleSpec, mocks::MockCronStore};

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
    fn decides_registration_from_missing_state() {
        let state = RegisterJobState::Missing;
        let command = RegisterJobCommand::new(job("backup")).unwrap();

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(JobEvent::JobRegistered {
                id: "backup".to_string(),
                spec: RegisteredJobSpec::from(job("backup")),
            }))
        );
    }

    #[test]
    fn rejects_registering_existing_job() {
        let state = RegisterJobState::Present;
        let command = RegisterJobCommand::new(job("backup")).unwrap();

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RegisterJobDecisionError::AlreadyRegistered { .. }
        ));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        run(&store, RegisterJobCommand::new(job("backup")).unwrap())
            .await
            .unwrap();

        let snapshot = store
            .get_job(GetJobCommand {
                id: JobId::parse("backup").unwrap(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.payload, job("backup"));

        let command_snapshot = store
            .read_command_snapshot::<RegisterJobState>(
                SNAPSHOT_STORE_CONFIG,
                &JobId::parse("backup").unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            command_snapshot,
            Snapshot::new(1, RegisterJobState::Present)
        );
    }
}
