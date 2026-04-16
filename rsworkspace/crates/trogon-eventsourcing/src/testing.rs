#![allow(clippy::panic)]

use std::{collections::HashMap, fmt::Debug, marker::PhantomData};

use crate::{CommandState, Decide, Decision, NonEmpty, StreamCommand};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThenEvents<Event> {
    stream_id: String,
    given: Vec<Event>,
    then: NonEmpty<Event>,
}

impl<Event> ThenEvents<Event> {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn given_events(&self) -> &[Event] {
        &self.given
    }

    pub fn then_events(&self) -> &[Event] {
        self.then.as_slice()
    }
}

impl<Event> ThenEvents<Event>
where
    Event: Clone,
{
    pub fn history(&self) -> Vec<Event> {
        let mut history = self.given.clone();
        history.extend(self.then.as_slice().iter().cloned());
        history
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThenError<Event, Error> {
    stream_id: String,
    given: Vec<Event>,
    error: Error,
}

impl<Event, Error> ThenError<Event, Error> {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn given_events(&self) -> &[Event] {
        &self.given
    }

    pub fn error(&self) -> &Error {
        &self.error
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline<Event> {
    cases: Vec<ThenEvents<Event>>,
    histories: HashMap<String, Vec<Event>>,
}

impl<Event> Default for Timeline<Event> {
    fn default() -> Self {
        Self {
            cases: Vec::new(),
            histories: HashMap::new(),
        }
    }
}

impl<Event> Timeline<Event>
where
    Event: Clone + Debug + PartialEq,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn given<I>(mut self, cases: I) -> Self
    where
        I: IntoIterator<Item = ThenEvents<Event>>,
    {
        for case in cases {
            self.push(case);
        }
        self
    }

    pub fn then_events<I>(self, expected: I) -> Self
    where
        I: IntoIterator<Item = Event>,
    {
        let expected: Vec<_> = expected.into_iter().collect();
        assert_eq!(
            self.events().as_slice(),
            expected.as_slice(),
            "timeline then-events did not match expectation"
        );
        self
    }

    pub fn then_stream<I>(self, stream_id: &str, expected: I) -> Self
    where
        I: IntoIterator<Item = Event>,
    {
        let expected: Vec<_> = expected.into_iter().collect();
        assert_eq!(
            self.stream_history(stream_id).as_slice(),
            expected.as_slice(),
            "timeline stream history did not match expectation for stream {:?}",
            stream_id
        );
        self
    }

    pub fn events(&self) -> Vec<Event> {
        self.cases
            .iter()
            .flat_map(|case| case.then_events().iter().cloned())
            .collect()
    }

    pub fn stream_history(&self, stream_id: &str) -> Vec<Event> {
        self.histories.get(stream_id).cloned().unwrap_or_default()
    }

    fn push(&mut self, case: ThenEvents<Event>) {
        let current = self.stream_history(case.stream_id());
        assert_eq!(
            current.as_slice(),
            case.given_events(),
            "timeline given history did not match prior then outputs for stream {:?}",
            case.stream_id()
        );

        let mut next = current;
        next.extend(case.then_events().iter().cloned());
        self.histories.insert(case.stream_id().to_string(), next);
        self.cases.push(case);
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Start;

#[doc(hidden)]
pub struct Given<Event> {
    history: Vec<Event>,
}

#[doc(hidden)]
pub struct When<Event, State, Command> {
    history: Vec<Event>,
    state: State,
    command: Command,
}

#[must_use = "test cases must be completed with .then(...)"]
pub struct TestCase<C, Stage = Start> {
    marker: PhantomData<fn() -> C>,
    stage: Stage,
    completed: bool,
}

impl<C> TestCase<C, Start> {
    pub const fn new(_decider: Decider<C>) -> Self {
        Self {
            marker: PhantomData,
            stage: Start,
            completed: false,
        }
    }
}

impl<C> TestCase<C, Start>
where
    C: CommandState + Decide<C::State, C::Event>,
{
    pub fn given<I>(mut self, events: I) -> TestCase<C, Given<C::Event>>
    where
        I: IntoIterator<Item = C::Event>,
    {
        self.completed = true;
        TestCase {
            marker: PhantomData,
            stage: Given {
                history: events.into_iter().collect(),
            },
            completed: false,
        }
    }
}

impl<C> TestCase<C, Given<C::Event>>
where
    C: CommandState + Decide<C::State, C::Event>,
    C::Event: Clone + Debug,
    C::DomainError: Debug,
{
    pub fn when(mut self, command: C) -> TestCase<C, When<C::Event, C::State, C>> {
        self.completed = true;
        let mut state = C::initial_state();
        let history = std::mem::take(&mut self.stage.history);

        for (index, event) in history.iter().cloned().enumerate() {
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
            marker: PhantomData,
            stage: When {
                history,
                state,
                command,
            },
            completed: false,
        }
    }
}

impl<C> TestCase<C, When<C::Event, C::State, C>>
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
    C::StreamId: std::fmt::Display,
{
    pub fn then<E>(mut self, expectation: E) -> E::Output
    where
        E: ThenExpectation<C>,
    {
        self.completed = true;
        let actual = C::decide(&self.stage.state, &self.stage.command);
        expectation.assert_matches(
            std::mem::take(&mut self.stage.history),
            &self.stage.command,
            actual,
        )
    }
}

impl<C, Stage> Drop for TestCase<C, Stage> {
    fn drop(&mut self) {
        assert!(
            self.completed || std::thread::panicking(),
            "test cases must be completed with .then(...)"
        );
    }
}

pub trait ThenExpectation<C>: private::Sealed<C>
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::StreamId: std::fmt::Display,
{
    type Output;

    fn assert_matches(
        self,
        given: Vec<C::Event>,
        command: &C,
        actual: Result<Decision<C::Event>, C::Error>,
    ) -> Self::Output;
}

impl<C> ThenExpectation<C> for NonEmpty<C::Event>
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
    C::StreamId: std::fmt::Display,
{
    type Output = ThenEvents<C::Event>;

    fn assert_matches(
        self,
        given: Vec<C::Event>,
        command: &C,
        actual: Result<Decision<C::Event>, C::Error>,
    ) -> Self::Output {
        assert_events_match::<C>(self.into_vec(), given, command, actual)
    }
}

impl<C> ThenExpectation<C> for Vec<C::Event>
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
    C::StreamId: std::fmt::Display,
{
    type Output = ThenEvents<C::Event>;

    fn assert_matches(
        self,
        given: Vec<C::Event>,
        command: &C,
        actual: Result<Decision<C::Event>, C::Error>,
    ) -> Self::Output {
        assert!(
            !self.is_empty(),
            "expected events in then(...), but event expectations must be non-empty"
        );
        assert_events_match::<C>(self, given, command, actual)
    }
}

impl<C, const N: usize> ThenExpectation<C> for [C::Event; N]
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
    C::StreamId: std::fmt::Display,
{
    type Output = ThenEvents<C::Event>;

    fn assert_matches(
        self,
        given: Vec<C::Event>,
        command: &C,
        actual: Result<Decision<C::Event>, C::Error>,
    ) -> Self::Output {
        assert!(
            N > 0,
            "expected events in then(...), but event expectations must be non-empty"
        );
        assert_events_match::<C>(Vec::from(self), given, command, actual)
    }
}

impl<C> ThenExpectation<C> for ExpectedError<C::Error>
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
    C::StreamId: std::fmt::Display,
{
    type Output = ThenError<C::Event, C::Error>;

    fn assert_matches(
        self,
        given: Vec<C::Event>,
        command: &C,
        actual: Result<Decision<C::Event>, C::Error>,
    ) -> Self::Output {
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

        ThenError {
            stream_id: command.stream_id().to_string(),
            given,
            error: self.0,
        }
    }
}

fn assert_events_match<C>(
    expected: Vec<C::Event>,
    given: Vec<C::Event>,
    command: &C,
    actual: Result<Decision<C::Event>, C::Error>,
) -> ThenEvents<C::Event>
where
    C: CommandState + Decide<C::State, C::Event> + StreamCommand,
    C::Event: Clone + PartialEq + Debug,
    C::Error: PartialEq + Debug,
    C::StreamId: std::fmt::Display,
{
    match actual {
        Ok(Decision::Event(events)) => {
            assert_eq!(
                events.as_slice(),
                expected.as_slice(),
                "then(...) emitted events did not match expectation"
            );
            ThenEvents {
                stream_id: command.stream_id().to_string(),
                given,
                then: events,
            }
        }
        Err(error) => panic!(
            "then(...) expected events, but decide(...) returned an error:\nexpected events = {:?}\nactual error = {:?}",
            expected, error,
        ),
    }
}

mod private {
    use crate::{CommandState, Decide, NonEmpty};

    use super::ExpectedError;

    pub trait Sealed<C>
    where
        C: CommandState + Decide<C::State, C::Event>,
    {
    }

    impl<C> Sealed<C> for NonEmpty<C::Event> where C: CommandState + Decide<C::State, C::Event> {}

    impl<C> Sealed<C> for Vec<C::Event> where C: CommandState + Decide<C::State, C::Event> {}

    impl<C, const N: usize> Sealed<C> for [C::Event; N] where
        C: CommandState + Decide<C::State, C::Event>
    {
    }

    impl<C> Sealed<C> for ExpectedError<C::Error> where C: CommandState + Decide<C::State, C::Event> {}
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

    impl CommandState for TestCommand {
        type State = TestState;
        type Event = TestEvent;
        type DomainError = TestDomainError;

        fn initial_state() -> Self::State {
            TestState::Missing
        }

        fn evolve(
            state: Self::State,
            event: Self::Event,
        ) -> Result<Self::State, Self::DomainError> {
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
        let register = TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);

        assert_eq!(register.stream_id(), "alpha");
        assert_eq!(register.given_events(), []);
        assert_eq!(
            register.then_events(),
            [TestEvent::Registered {
                id: "alpha".to_string(),
            }]
        );
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
        let _ = TestCase::new(decider::<TestCommand>())
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
        let error = TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::remove("alpha"))
            .then(expect_error(TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            }));

        assert_eq!(error.stream_id(), "alpha");
        assert_eq!(error.given_events(), []);
        assert_eq!(
            error.error(),
            &TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            }
        );
    }

    #[test]
    fn timeline_matches_then_outputs_against_next_given_for_same_stream() {
        let register = TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);

        let disable = TestCase::new(decider::<TestCommand>())
            .given(register.history())
            .when(TestCommand::disable("alpha"))
            .then([TestEvent::Disabled {
                id: "alpha".to_string(),
            }]);

        Timeline::new()
            .given([register, disable])
            .then_stream(
                "alpha",
                [
                    TestEvent::Registered {
                        id: "alpha".to_string(),
                    },
                    TestEvent::Disabled {
                        id: "alpha".to_string(),
                    },
                ],
            )
            .then_events([
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
                TestEvent::Disabled {
                    id: "alpha".to_string(),
                },
            ]);
    }

    #[test]
    fn timeline_keeps_command_streams_separate() {
        let alpha = TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);

        let beta = TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("beta"))
            .then([TestEvent::Registered {
                id: "beta".to_string(),
            }]);

        Timeline::new()
            .given([alpha, beta])
            .then_stream(
                "alpha",
                [TestEvent::Registered {
                    id: "alpha".to_string(),
                }],
            )
            .then_stream(
                "beta",
                [TestEvent::Registered {
                    id: "beta".to_string(),
                }],
            );
    }

    #[test]
    fn timeline_rejects_mismatched_given_history_for_command_stream() {
        let register = TestCase::new(decider::<TestCommand>())
            .given([])
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);

        let invalid_disable = TestCase::new(decider::<TestCommand>())
            .given([
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
                TestEvent::Removed {
                    id: "alpha".to_string(),
                },
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
            ])
            .when(TestCommand::disable("alpha"))
            .then([TestEvent::Disabled {
                id: "alpha".to_string(),
            }]);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _ = Timeline::new().given([register, invalid_disable]);
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("timeline given history did not match prior then outputs"));
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
    fn unfinished_test_case_panics_on_drop() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _case = TestCase::new(decider::<TestCommand>()).given([]);
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("test cases must be completed with .then(...)"));
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
