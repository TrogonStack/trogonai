use std::fmt;

use trogon_eventsourcing::{Decide, Decision, NonEmpty, StreamCommand, decide};

use crate::{
    JobId, JobSpec, JobWriteCondition, ResolvedJobSpec,
    commands::{CommandRuntime, Evolve, catch_up_command_state},
    error::CronError,
    events::{JobEvent, JobEventData, RegisteredJobSpec},
};

#[derive(Debug, Clone)]
pub struct RegisterJobCommand {
    id: JobId,
    job: ResolvedJobSpec,
    write_condition: JobWriteCondition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn new(spec: JobSpec, write_condition: JobWriteCondition) -> Result<Self, CronError> {
        let id = JobId::parse(&spec.id).map_err(|source| {
            CronError::event_source(
                "failed to build register job command from validated spec",
                source,
            )
        })?;
        Ok(Self {
            id,
            job: ResolvedJobSpec::try_from(&spec)?,
            write_condition,
        })
    }

    pub fn job(&self) -> &ResolvedJobSpec {
        &self.job
    }

    pub const fn write_condition(&self) -> JobWriteCondition {
        self.write_condition
    }

    pub fn state_from_snapshot(
        &self,
        snapshot: Option<&trogon_eventsourcing::Snapshot<JobSpec>>,
    ) -> Result<RegisterJobState, CronError> {
        match snapshot {
            None => Ok(RegisterJobState::Missing),
            Some(snapshot) if snapshot.payload.id == self.stream_id().as_str() => {
                Ok(RegisterJobState::Present)
            }
            Some(snapshot) => Err(CronError::event_source(
                "failed to decode current job snapshot into register-job state",
                std::io::Error::other(format!(
                    "expected '{}' but snapshot carried '{}'",
                    self.stream_id(),
                    snapshot.payload.id
                )),
            )),
        }
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

impl Evolve for RegisterJobCommand {
    type State = RegisterJobState;

    fn evolve(_state: Self::State, event: JobEvent) -> Result<Self::State, CronError> {
        match event {
            JobEvent::JobRegistered { .. } | JobEvent::JobStateChanged { .. } => {
                Ok(RegisterJobState::Present)
            }
            JobEvent::JobRemoved { .. } => Ok(RegisterJobState::Missing),
        }
    }
}

pub async fn run<R>(runtime: &R, command: RegisterJobCommand) -> Result<(), CronError>
where
    R: CommandRuntime,
{
    let id = command.stream_id().to_string();
    let current_snapshot = runtime.load_job_snapshot(command.stream_id()).await?;
    let current_state = command.state_from_snapshot(current_snapshot.as_ref())?;
    let (current_state, current_version) =
        catch_up_command_state(runtime, &command, current_snapshot.as_ref(), current_state).await?;

    let events = match decide(&current_state, &command) {
        Ok(Decision::Event(events)) => events,
        Ok(_) => {
            return Err(CronError::event_source(
                "failed to apply job registration to current stream state",
                std::io::Error::other("unsupported decision variant"),
            ));
        }
        Err(RegisterJobDecisionError::AlreadyRegistered { .. }) => {
            return Err(CronError::OptimisticConcurrencyConflict {
                id: id.clone(),
                expected: command.write_condition(),
                current_version,
            });
        }
    };

    let events = events.try_map(JobEventData::new)?;
    runtime
        .append_job_events(command.stream_id(), command.write_condition(), events)
        .await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use trogon_eventsourcing::{Decision, NonEmpty, decide};

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
        let command =
            RegisterJobCommand::new(job("backup"), JobWriteCondition::MustNotExist).unwrap();

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
        let command =
            RegisterJobCommand::new(job("backup"), JobWriteCondition::MustNotExist).unwrap();

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            RegisterJobDecisionError::AlreadyRegistered { .. }
        ));
    }

    #[tokio::test]
    async fn run_registers_job_in_store() {
        let store = MockCronStore::new();

        run(
            &store,
            RegisterJobCommand::new(job("backup"), JobWriteCondition::MustNotExist).unwrap(),
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
        assert_eq!(snapshot.payload, job("backup"));
    }
}
