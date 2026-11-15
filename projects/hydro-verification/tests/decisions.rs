use hydro_lang::prelude::*;
use trogon_decider::{Decider, Events};
use trogon_scheduler_domain::domain::{
    Delivery, MessageContent, Schedule, ScheduleEventStatus, ScheduleHeaders, ScheduleId,
    ScheduleMessage,
};
use trogon_scheduler_domain::{
    CreateSchedule, CreateScheduleDecideError, PauseSchedule, PauseScheduleError, RemoveSchedule,
    RemoveScheduleError, ResumeSchedule, ResumeScheduleError, evolve, initial_state,
};
use trogonai_hydro_verification::{Command, Scheduler, client_orders};
use trogonai_proto::scheduler::schedules::{ScheduleEventCase, state_v1, v1};

fn schedule_id() -> ScheduleId {
    uuid::Uuid::from_u128(1).into()
}

fn create() -> CreateSchedule {
    CreateSchedule {
        id: schedule_id(),
        status: ScheduleEventStatus::Scheduled,
        schedule: Schedule::every(std::time::Duration::from_secs(30)).unwrap(),
        delivery: Delivery::nats_event("scheduler.test").unwrap(),
        message: ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::default(),
        },
    }
}

fn status(state: &state_v1::State) -> state_v1::StateValue {
    state
        .state
        .as_ref()
        .and_then(|value| value.as_known())
        .unwrap()
}

struct History {
    state: state_v1::State,
    events: Vec<v1::ScheduleEvent>,
}

impl History {
    fn empty() -> Self {
        Self {
            state: initial_state(),
            events: Vec::new(),
        }
    }

    fn created() -> Self {
        let mut history = Self::empty();
        history.apply(Command::Create);
        history
    }

    fn execute<C>(
        &mut self,
        command: C,
        reject: impl FnOnce(C::DecideError),
    ) -> Option<Events<v1::ScheduleEvent>>
    where
        C: Decider<State = state_v1::State, Event = v1::ScheduleEvent>,
    {
        match C::decide(&self.state, &command) {
            Ok(decision) => {
                let (next, events) = decision.handle(self.state.clone(), &command).unwrap();
                self.state = next;
                self.events.extend(events.iter().cloned());
                Some(events)
            }
            Err(error) => {
                reject(error);
                None
            }
        }
    }

    fn apply(&mut self, command: Command) {
        use state_v1::StateValue::*;

        let before = self.state.clone();
        let before_status = status(&before);
        let before_events = self.events.len();
        let id = schedule_id();
        let events = match command {
            Command::Create => self.execute(create(), |error| match before_status {
                STATE_VALUE_DELETED => {
                    assert_eq!(error, CreateScheduleDecideError::ScheduleDeleted { id });
                }
                STATE_VALUE_PRESENT_ENABLED | STATE_VALUE_PRESENT_DISABLED => {
                    assert_eq!(error, CreateScheduleDecideError::AlreadyExists { id });
                }
                _ => panic!("unexpected create rejection: {error:?}"),
            }),
            Command::Pause => self.execute(PauseSchedule::new(id.clone()), |error| {
                let expected = match before_status {
                    STATE_VALUE_MISSING => PauseScheduleError::ScheduleNotFound { id },
                    STATE_VALUE_DELETED => PauseScheduleError::ScheduleDeleted { id },
                    STATE_VALUE_PRESENT_DISABLED => PauseScheduleError::AlreadyPaused { id },
                    _ => panic!("unexpected pause rejection: {error:?}"),
                };
                assert_eq!(error, expected);
            }),
            Command::Resume => self.execute(ResumeSchedule::new(id.clone()), |error| {
                let expected = match before_status {
                    STATE_VALUE_MISSING => ResumeScheduleError::ScheduleNotFound { id },
                    STATE_VALUE_DELETED => ResumeScheduleError::ScheduleDeleted { id },
                    STATE_VALUE_PRESENT_ENABLED => ResumeScheduleError::AlreadyActive { id },
                    _ => panic!("unexpected resume rejection: {error:?}"),
                };
                assert_eq!(error, expected);
            }),
            Command::Remove => self.execute(RemoveSchedule::new(id.clone()), |error| {
                let expected = match before_status {
                    STATE_VALUE_MISSING => RemoveScheduleError::ScheduleNotFound { id },
                    STATE_VALUE_DELETED => RemoveScheduleError::ScheduleDeleted { id },
                    _ => panic!("unexpected remove rejection: {error:?}"),
                };
                assert_eq!(error, expected);
            }),
        };

        if let Some(events) = events {
            assert_ne!(
                before_status, STATE_VALUE_DELETED,
                "{command:?} accepted after removal"
            );
            assert_eq!(events.len(), 1);
            let event = events.iter().next().unwrap();
            let expected_status = match (command, event.event.as_ref().unwrap()) {
                (Command::Create, ScheduleEventCase::ScheduleCreated(created)) => {
                    assert_eq!(before_status, STATE_VALUE_MISSING);
                    assert_eq!(created.schedule_id, schedule_id().to_string());
                    STATE_VALUE_PRESENT_ENABLED
                }
                (Command::Pause, ScheduleEventCase::SchedulePaused(paused)) => {
                    assert_eq!(before_status, STATE_VALUE_PRESENT_ENABLED);
                    assert_eq!(paused.schedule_id, schedule_id().to_string());
                    STATE_VALUE_PRESENT_DISABLED
                }
                (Command::Resume, ScheduleEventCase::ScheduleResumed(resumed)) => {
                    assert_eq!(before_status, STATE_VALUE_PRESENT_DISABLED);
                    assert_eq!(resumed.schedule_id, schedule_id().to_string());
                    STATE_VALUE_PRESENT_ENABLED
                }
                (Command::Remove, ScheduleEventCase::ScheduleRemoved(removed)) => {
                    assert_eq!(removed.schedule_id, schedule_id().to_string());
                    STATE_VALUE_DELETED
                }
                _ => panic!("unexpected event for {command:?}: {event:?}"),
            };
            assert_eq!(status(&self.state), expected_status);
        } else {
            assert_eq!(self.state, before);
            assert_eq!(self.events.len(), before_events);
        }

        let replayed = self
            .events
            .iter()
            .try_fold(initial_state(), evolve)
            .unwrap();
        assert_eq!(self.state, replayed);
    }
}

fn explore(
    left_commands: &[Command],
    right_commands: &[Command],
    check: impl Fn(&[Command]) + std::panic::RefUnwindSafe,
) -> usize {
    let mut flow = FlowBuilder::new();
    let process = flow.process::<Scheduler>();
    let (left, left_stream) = process.sim_input();
    let (right, right_stream) = process.sim_input();
    let output = client_orders(left_stream, right_stream).sim_output();
    flow.sim().exhaustive(async || {
        left.send_many(left_commands.iter().copied());
        right.send_many(right_commands.iter().copied());
        let commands: Vec<Command> = output.collect().await;
        assert_eq!(commands.len(), left_commands.len() + right_commands.len());
        check(&commands);
    })
}

#[test]
fn removal_stays_terminal_across_client_interleavings() {
    let left = [
        Command::Pause,
        Command::Resume,
        Command::Pause,
        Command::Resume,
    ];
    let right = [
        Command::Remove,
        Command::Create,
        Command::Pause,
        Command::Resume,
    ];
    let executions = explore(&left, &right, |commands| {
        let mut history = History::created();
        for command in commands {
            history.apply(*command);
        }
        assert_eq!(
            status(&history.state),
            state_v1::StateValue::STATE_VALUE_DELETED
        );
        assert_eq!(
            history
                .events
                .iter()
                .filter(|event| matches!(event.event, Some(ScheduleEventCase::ScheduleRemoved(_))))
                .count(),
            1,
        );
    });
    eprintln!("removal: {executions} Hydro executions of production deciders");
    assert!(executions > 1);
}

#[test]
fn duplicate_creation_keeps_one_event_history() {
    let commands = [Command::Create, Command::Pause, Command::Resume];
    let executions = explore(&commands, &commands, |ordered| {
        let mut history = History::empty();
        for command in ordered {
            history.apply(*command);
        }
        assert_eq!(
            status(&history.state),
            state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
        );
        assert_eq!(
            history
                .events
                .iter()
                .filter(|event| matches!(event.event, Some(ScheduleEventCase::ScheduleCreated(_))))
                .count(),
            1,
        );
    });
    eprintln!("duplicate creation: {executions} Hydro executions of production deciders");
    assert!(executions > 1);
}

#[test]
fn missing_schedule_commands_return_typed_rejections() {
    let left = [Command::Pause, Command::Resume];
    let right = [Command::Remove, Command::Pause, Command::Resume];
    let executions = explore(&left, &right, |commands| {
        let mut history = History::empty();
        for command in commands {
            history.apply(*command);
        }
        assert!(history.events.is_empty());
        assert_eq!(history.state, initial_state());
    });
    eprintln!("missing schedule: {executions} Hydro executions of production deciders");
    assert!(executions > 1);
}
