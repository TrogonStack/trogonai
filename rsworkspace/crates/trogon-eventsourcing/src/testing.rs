#![allow(clippy::panic)]

use std::{fmt::Debug, marker::PhantomData};

use crate::{CommandStateModel, Decide, Decision, NonEmpty};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Decider<C>(PhantomData<fn() -> C>);

pub const fn decider<C>() -> Decider<C> {
    Decider(PhantomData)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedError<E>(E);

pub const fn expect_error<E>(error: E) -> ExpectedError<E> {
    ExpectedError(error)
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Start;

#[doc(hidden)]
pub struct Given<Event> {
    history: Vec<Event>,
}

#[doc(hidden)]
pub struct When<State, Command> {
    state: State,
    command: Command,
}

pub struct TestCase<C, Stage = Start> {
    decider: Decider<C>,
    stage: Stage,
}

impl<C> TestCase<C, Start> {
    pub const fn new(decider: Decider<C>) -> Self {
        Self {
            decider,
            stage: Start,
        }
    }
}

impl<C> TestCase<C, Start>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
{
    pub fn given<I>(self, events: I) -> TestCase<C, Given<C::Event>>
    where
        I: IntoIterator<Item = C::Event>,
    {
        TestCase {
            decider: self.decider,
            stage: Given {
                history: events.into_iter().collect(),
            },
        }
    }
}

impl<C> TestCase<C, Given<C::Event>>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
    C::Event: Clone + Debug,
    C::DomainError: Debug,
{
    pub fn when(self, command: C) -> TestCase<C, When<C::State, C>> {
        let mut state = C::initial_state();

        for (index, event) in self.stage.history.into_iter().enumerate() {
            state = match C::evolve(state, event.clone()) {
                Ok(next) => next,
                Err(error) => panic!(
                    "Given history could not be replayed at event {}:\nevent = {:?}\nerror = {:?}",
                    index + 1,
                    event,
                    error,
                ),
            };
        }

        TestCase {
            decider: self.decider,
            stage: When { state, command },
        }
    }
}

impl<C> TestCase<C, When<C::State, C>>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
{
    pub fn then<E>(self, expectation: E)
    where
        E: ThenExpectation<C>,
    {
        expectation.assert_matches(C::decide(&self.stage.state, &self.stage.command));
    }
}

pub trait ThenExpectation<C>: private::Sealed<C>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
{
    fn assert_matches(self, actual: Result<Decision<C::Event>, C::Error>);
}

impl<C> ThenExpectation<C> for NonEmpty<C::Event>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
{
    fn assert_matches(self, actual: Result<Decision<C::Event>, C::Error>) {
        assert_events_match(self.into_vec(), actual);
    }
}

impl<C> ThenExpectation<C> for Vec<C::Event>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
{
    fn assert_matches(self, actual: Result<Decision<C::Event>, C::Error>) {
        assert!(
            !self.is_empty(),
            "expected events in then(...), but event expectations must be non-empty"
        );
        assert_events_match(self, actual);
    }
}

impl<C, const N: usize> ThenExpectation<C> for [C::Event; N]
where
    C: CommandStateModel + Decide<C::State, C::Event>,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
{
    fn assert_matches(self, actual: Result<Decision<C::Event>, C::Error>) {
        assert!(
            N > 0,
            "expected events in then(...), but event expectations must be non-empty"
        );
        assert_events_match(Vec::from(self), actual);
    }
}

impl<C> ThenExpectation<C> for ExpectedError<C::Error>
where
    C: CommandStateModel + Decide<C::State, C::Event>,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
{
    fn assert_matches(self, actual: Result<Decision<C::Event>, C::Error>) {
        match actual {
            Err(error) => assert_eq!(
                error, self.0,
                "then(...) expected an exact domain error, but the error did not match"
            ),
            Ok(Decision::Event(events)) => panic!(
                "then(...) expected an error, but decide(...) emitted events:\nexpected error = {:?}\nactual events = {:?}",
                self.0,
                events.as_slice(),
            ),
        }
    }
}

fn assert_events_match<Event, Error>(
    expected: Vec<Event>,
    actual: Result<Decision<Event>, Error>,
) where
    Event: PartialEq + Debug,
    Error: PartialEq + Debug,
{
    match actual {
        Ok(Decision::Event(events)) => assert_eq!(
            events.as_slice(),
            expected.as_slice(),
            "then(...) emitted events did not match expectation"
        ),
        Err(error) => panic!(
            "then(...) expected events, but decide(...) returned an error:\nexpected events = {:?}\nactual error = {:?}",
            expected,
            error,
        ),
    }
}

mod private {
    use crate::{CommandStateModel, Decide, NonEmpty};

    use super::ExpectedError;

    pub trait Sealed<C>
    where
        C: CommandStateModel + Decide<C::State, C::Event>,
    {
    }

    impl<C> Sealed<C> for NonEmpty<C::Event>
    where
        C: CommandStateModel + Decide<C::State, C::Event>,
    {
    }

    impl<C> Sealed<C> for Vec<C::Event>
    where
        C: CommandStateModel + Decide<C::State, C::Event>,
    {
    }

    impl<C, const N: usize> Sealed<C> for [C::Event; N]
    where
        C: CommandStateModel + Decide<C::State, C::Event>,
    {
    }

    impl<C> Sealed<C> for ExpectedError<C::Error>
    where
        C: CommandStateModel + Decide<C::State, C::Event>,
    {
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;
    use crate::{Decision, StreamCommand};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestState {
        Missing,
        Present { enabled: bool },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestEvent {
        Registered { id: String },
        Disabled { id: String },
        Removed { id: String },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestDomainError {
        MissingJobForDisable { id: String },
        MissingJobForRemoval { id: String },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestCommandError {
        AlreadyRegistered { id: String },
        JobNotFound { id: String },
        AlreadyDisabled { id: String },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestAction {
        Register,
        Disable,
        Remove,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestCommand {
        id: &'static str,
        action: TestAction,
    }

    impl TestCommand {
        fn register(id: &'static str) -> Self {
            Self {
                id,
                action: TestAction::Register,
            }
        }

        fn disable(id: &'static str) -> Self {
            Self {
                id,
                action: TestAction::Disable,
            }
        }

        fn remove(id: &'static str) -> Self {
            Self {
                id,
                action: TestAction::Remove,
            }
        }
    }

    impl StreamCommand for TestCommand {
        type StreamId = str;

        fn stream_id(&self) -> &Self::StreamId {
            self.id
        }
    }

    impl CommandStateModel for TestCommand {
        type State = TestState;
        type Event = TestEvent;
        type DomainError = TestDomainError;

        fn initial_state() -> Self::State {
            TestState::Missing
        }

        fn evolve(state: Self::State, event: Self::Event) -> Result<Self::State, Self::DomainError> {
            match (state, event) {
                (TestState::Missing, TestEvent::Registered { .. }) => {
                    Ok(TestState::Present { enabled: true })
                }
                (TestState::Missing, TestEvent::Disabled { id }) => {
                    Err(TestDomainError::MissingJobForDisable { id })
                }
                (TestState::Missing, TestEvent::Removed { id }) => {
                    Err(TestDomainError::MissingJobForRemoval { id })
                }
                (TestState::Present { .. }, TestEvent::Registered { id }) => {
                    Err(TestDomainError::MissingJobForDisable { id })
                }
                (TestState::Present { .. }, TestEvent::Disabled { .. }) => {
                    Ok(TestState::Present { enabled: false })
                }
                (TestState::Present { .. }, TestEvent::Removed { .. }) => Ok(TestState::Missing),
            }
        }
    }

    impl Decide<TestState, TestEvent> for TestCommand {
        type Error = TestCommandError;

        fn decide(state: &TestState, command: &Self) -> Result<Decision<TestEvent>, Self::Error> {
            match (&command.action, state) {
                (TestAction::Register, TestState::Missing) => {
                    Ok(Decision::Event(NonEmpty::one(TestEvent::Registered {
                        id: command.id.to_string(),
                    })))
                }
                (TestAction::Register, TestState::Present { .. }) => {
                    Err(TestCommandError::AlreadyRegistered {
                        id: command.id.to_string(),
                    })
                }
                (TestAction::Disable, TestState::Missing) => Err(TestCommandError::JobNotFound {
                    id: command.id.to_string(),
                }),
                (TestAction::Disable, TestState::Present { enabled: false }) => {
                    Err(TestCommandError::AlreadyDisabled {
                        id: command.id.to_string(),
                    })
                }
                (TestAction::Disable, TestState::Present { enabled: true }) => {
                    Ok(Decision::Event(NonEmpty::one(TestEvent::Disabled {
                        id: command.id.to_string(),
                    })))
                }
                (TestAction::Remove, TestState::Missing) => Err(TestCommandError::JobNotFound {
                    id: command.id.to_string(),
                }),
                (TestAction::Remove, TestState::Present { .. }) => {
                    Ok(Decision::Event(NonEmpty::one(TestEvent::Removed {
                        id: command.id.to_string(),
                    })))
                }
            }
        }
    }

    #[test]
    fn given_empty_history_when_command_then_array_events_succeeds() {
        TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn given_history_when_command_then_array_events_rehydrates_state() {
        TestCase::new(decider::<TestCommand>())
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::disable("alpha"))
            .then([TestEvent::Disabled {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn given_history_when_command_then_exact_error_succeeds() {
        TestCase::new(decider::<TestCommand>())
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::register("alpha"))
            .then(expect_error(TestCommandError::AlreadyRegistered {
                id: "alpha".to_string(),
            }));
    }

    #[test]
    #[should_panic(expected = "Given history could not be replayed at event 1")]
    fn invalid_history_in_given_panics_during_replay() {
        TestCase::new(decider::<TestCommand>())
            .given([TestEvent::Removed {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::register("alpha"));
    }

    #[test]
    fn then_vec_events_works() {
        TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then(vec![TestEvent::Registered {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn then_non_empty_events_works() {
        TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then(NonEmpty::one(TestEvent::Registered {
                id: "alpha".to_string(),
            }));
    }

    #[test]
    fn then_expected_error_wrapper_works() {
        TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::remove("alpha"))
            .then(expect_error(TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            }));
    }

    #[test]
    #[should_panic(expected = "event expectations must be non-empty")]
    fn then_empty_event_vec_panics() {
        TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then(Vec::<TestEvent>::new());
    }

    #[test]
    fn event_mismatch_shows_expected_vs_actual() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::new(decider::<TestCommand>())
                .given([])
                .when(TestCommand::register("alpha"))
                .then([TestEvent::Removed {
                    id: "alpha".to_string(),
                }]);
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("then(...) emitted events did not match expectation"));
        assert!(message.contains("Registered"));
        assert!(message.contains("Removed"));
    }

    #[test]
    fn error_mismatch_shows_expected_vs_actual() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::new(decider::<TestCommand>())
                .given([])
                .when(TestCommand::register("alpha"))
                .then(expect_error(TestCommandError::JobNotFound {
                    id: "alpha".to_string(),
                }));
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("then(...) expected an error"));
        assert!(message.contains("JobNotFound"));
        assert!(message.contains("Registered"));
    }

    fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
        if let Some(message) = panic.downcast_ref::<String>() {
            return message.clone();
        }
        if let Some(message) = panic.downcast_ref::<&'static str>() {
            return (*message).to_string();
        }
        String::new()
    }
}
