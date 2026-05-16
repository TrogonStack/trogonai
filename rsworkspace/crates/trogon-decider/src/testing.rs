#![allow(clippy::panic)]
//! Test support for decider behavior.
//!
//! Enable the `test-support` feature to use this module from downstream crate tests.
//! The API is intentionally shaped around the event-sourcing test sentence:
//! given prior events, when a command is decided, then events or an error should result.
//!
//! In this module, [`History`] means replay input for one stream. It may be empty.
//! [`Events`] means emitted decision output and is non-empty by construction.
//!
//! # Command emits events
//!
//! ```
//! use trogon_decider::{Decider, Decision};
//! use trogon_decider::testing::TestCase;
//!
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! enum AccountState {
//!     Missing,
//!     Open,
//! }
//!
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! enum AccountEvent {
//!     Opened,
//! }
//!
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! enum AccountError {
//!     AlreadyOpen,
//! }
//!
//! impl std::fmt::Display for AccountError {
//!     fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         write!(formatter, "{self:?}")
//!     }
//! }
//!
//! impl std::error::Error for AccountError {}
//!
//! #[derive(Debug, Clone, PartialEq, Eq)]
//! struct OpenAccount {
//!     account_id: &'static str,
//! }
//!
//! impl Decider for OpenAccount {
//!     type StreamId = str;
//!     type State = AccountState;
//!     type Event = AccountEvent;
//!     type DecideError = AccountError;
//!     type EvolveError = AccountError;
//!
//!     fn stream_id(&self) -> &Self::StreamId {
//!         self.account_id
//!     }
//!
//!     fn initial_state() -> Self::State {
//!         AccountState::Missing
//!     }
//!
//!     fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
//!         match event {
//!             AccountEvent::Opened => Ok(AccountState::Open),
//!         }
//!     }
//!
//!     fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
//!         match state {
//!             AccountState::Missing => Ok(Decision::event(AccountEvent::Opened)),
//!             AccountState::Open => Err(AccountError::AlreadyOpen),
//!         }
//!     }
//! }
//!
//! TestCase::<OpenAccount>::new()
//!     .given_no_history()
//!     .when(OpenAccount { account_id: "account-1" })
//!     .then([AccountEvent::Opened]);
//! ```
//!
//! # Command rejects
//!
//! ```
//! # use trogon_decider::{Decider, Decision};
//! # use trogon_decider::testing::TestCase;
//! #
//! # #[derive(Debug, Clone, PartialEq, Eq)]
//! # enum AccountState {
//! #     Missing,
//! #     Open,
//! # }
//! #
//! # #[derive(Debug, Clone, PartialEq, Eq)]
//! # enum AccountEvent {
//! #     Opened,
//! # }
//! #
//! # #[derive(Debug, Clone, PartialEq, Eq)]
//! # enum AccountError {
//! #     AlreadyOpen,
//! # }
//! #
//! # impl std::fmt::Display for AccountError {
//! #     fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//! #         write!(formatter, "{self:?}")
//! #     }
//! # }
//! #
//! # impl std::error::Error for AccountError {}
//! #
//! # #[derive(Debug, Clone, PartialEq, Eq)]
//! # struct OpenAccount {
//! #     account_id: &'static str,
//! # }
//! #
//! # impl Decider for OpenAccount {
//! #     type StreamId = str;
//! #     type State = AccountState;
//! #     type Event = AccountEvent;
//! #     type DecideError = AccountError;
//! #     type EvolveError = AccountError;
//! #
//! #     fn stream_id(&self) -> &Self::StreamId { self.account_id }
//! #     fn initial_state() -> Self::State { AccountState::Missing }
//! #     fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
//! #         match event {
//! #             AccountEvent::Opened => Ok(AccountState::Open),
//! #         }
//! #     }
//! #     fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
//! #         match state {
//! #             AccountState::Missing => Ok(Decision::event(AccountEvent::Opened)),
//! #             AccountState::Open => Err(AccountError::AlreadyOpen),
//! #         }
//! #     }
//! # }
//! #
//! TestCase::<OpenAccount>::new()
//!     .given([AccountEvent::Opened])
//!     .when(OpenAccount { account_id: "account-1" })
//!     .then_error(AccountError::AlreadyOpen);
//! ```

use std::{fmt::Debug, marker::PhantomData, mem::ManuallyDrop, ptr};

use crate::{Decider, DecisionFailure, Events};

/// Prior events used to rebuild decider state in tests.
///
/// History is replay input, so it is allowed to be empty. Emitted events use
/// [`Events`] instead, which preserves the production non-empty batch invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History<Event>(Vec<Event>);

impl<Event> History<Event> {
    /// Creates an empty history.
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Builds history from replay events.
    pub fn from_events<I, E>(events: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<Event>,
    {
        Self(events.into_iter().map(Into::into).collect())
    }

    /// Borrows replay events as a slice.
    pub fn as_slice(&self) -> &[Event] {
        &self.0
    }

    /// Consumes the history and returns the underlying vector.
    pub fn into_vec(self) -> Vec<Event> {
        self.0
    }

    /// Iterates over replay events in order.
    pub fn iter(&self) -> impl Iterator<Item = &Event> {
        self.0.iter()
    }

    /// Number of replay events.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` when there are no replay events.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn extend<I, E>(&mut self, events: I)
    where
        I: IntoIterator<Item = E>,
        E: Into<Event>,
    {
        self.0.extend(events.into_iter().map(Into::into));
    }
}

impl<Event> AsRef<[Event]> for History<Event> {
    fn as_ref(&self) -> &[Event] {
        self.as_slice()
    }
}

impl<Event> IntoIterator for History<Event> {
    type Item = Event;
    type IntoIter = std::vec::IntoIter<Event>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<Event> From<Vec<Event>> for History<Event> {
    fn from(events: Vec<Event>) -> Self {
        Self(events)
    }
}

impl<Event, const N: usize> From<[Event; N]> for History<Event> {
    fn from(events: [Event; N]) -> Self {
        Self(Vec::from(events))
    }
}

impl<Event> FromIterator<Event> for History<Event> {
    fn from_iter<I>(events: I) -> Self
    where
        I: IntoIterator<Item = Event>,
    {
        Self(events.into_iter().collect())
    }
}

impl<Event> Default for History<Event> {
    fn default() -> Self {
        Self::new()
    }
}

/// A completed event expectation.
///
/// The value records the stream id, prior history, and events emitted by one
/// completed [`TestCase`]. Feed [`history`](Self::history) into the next case when a
/// test needs to model multiple commands against the same stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThenEvents<Event> {
    stream_id: String,
    given: History<Event>,
    then: Events<Event>,
}

impl<Event> ThenEvents<Event> {
    /// Stream id resolved from the command used in the test case.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Prior events supplied to the test case.
    pub fn given_events(&self) -> &[Event] {
        self.given.as_slice()
    }

    /// Events emitted by the command under test.
    pub fn then_events(&self) -> &[Event] {
        self.then.as_slice()
    }
}

impl<Event> ThenEvents<Event>
where
    Event: Clone,
{
    /// Full stream history after this case: the given events plus emitted events.
    pub fn history(&self) -> History<Event> {
        let mut history = self.given.clone();
        history.extend(self.then.as_slice().iter().cloned());
        history
    }
}

/// A completed error expectation.
///
/// The value records the stream id, prior history, and decision error from one
/// completed [`TestCase`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThenError<Event, Error> {
    stream_id: String,
    given: History<Event>,
    error: Error,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionEvaluationFailure<DecideError, EvolveError> {
    Decide(DecideError),
    Evolve(EvolveError),
}

#[doc(hidden)]
pub type DecisionEvaluation<C> = Result<
    Events<<C as Decider>::Event>,
    DecisionEvaluationFailure<<C as Decider>::DecideError, <C as Decider>::EvolveError>,
>;

impl<Event, Error> ThenError<Event, Error> {
    /// Stream id resolved from the command used in the test case.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Prior events supplied to the test case.
    pub fn given_events(&self) -> &[Event] {
        self.given.as_slice()
    }

    /// Error returned by the command under test.
    pub fn error(&self) -> &Error {
        &self.error
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Start;

#[doc(hidden)]
pub struct Given<Event> {
    history: History<Event>,
}

#[doc(hidden)]
pub struct NoHistory;

#[doc(hidden)]
pub struct When<Event, State, Command> {
    history: History<Event>,
    state: State,
    expected_state: Option<StateAssertion<State>>,
    command: Command,
}

type StateAssertion<State> = Box<dyn FnOnce(&State)>;

/// Given-when-then test case for one [`Decider`] command.
///
/// `TestCase` uses typestate so the chain must be completed in the supported order:
/// choose prior history, provide the command, then assert emitted events or a decision
/// error. Invalid chains fail to compile in the trybuild tests.
#[must_use = "test cases must be completed with .then(...) or .then_error(...)"]
pub struct TestCase<C, Stage = Start> {
    marker: PhantomData<fn() -> C>,
    stage: Stage,
    completed: bool,
}

impl<C> TestCase<C, Start> {
    /// Starts a test case for decider command `C`.
    pub const fn new() -> Self {
        Self {
            marker: PhantomData,
            stage: Start,
            completed: false,
        }
    }
}

impl<C> Default for TestCase<C, Start> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, Stage> TestCase<C, Stage> {
    fn complete_into_stage(mut self) -> Stage {
        self.completed = true;
        let this = ManuallyDrop::new(self);
        unsafe { ptr::read(&this.stage) }
    }
}

impl<C> TestCase<C, Start>
where
    C: Decider,
{
    /// Starts from the decider's initial state with no prior events.
    pub fn given_no_history(mut self) -> TestCase<C, NoHistory> {
        self.completed = true;
        TestCase {
            marker: PhantomData,
            stage: NoHistory,
            completed: false,
        }
    }

    /// Starts from prior events replayed through [`Decider::evolve`].
    pub fn given<I, E>(mut self, events: I) -> TestCase<C, Given<C::Event>>
    where
        I: IntoIterator<Item = E>,
        E: Into<C::Event>,
    {
        self.completed = true;
        TestCase {
            marker: PhantomData,
            stage: Given {
                history: History::from_events(events),
            },
            completed: false,
        }
    }
}

impl<C> TestCase<C, Given<C::Event>>
where
    C: Decider,
{
    /// Appends more prior events before the command is evaluated.
    pub fn given<I, E>(mut self, events: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<C::Event>,
    {
        self.stage.history.extend(events);
        self
    }
}

impl<C> TestCase<C, NoHistory>
where
    C: Decider,
    C::Event: Clone + Debug,
    C::EvolveError: Debug,
{
    /// Provides the command under test.
    pub fn when(mut self, command: C) -> TestCase<C, When<C::Event, C::State, C>> {
        self.completed = true;
        TestCase {
            marker: PhantomData,
            stage: When {
                history: History::new(),
                state: C::initial_state(),
                expected_state: None,
                command,
            },
            completed: false,
        }
    }
}

impl<C> TestCase<C, Given<C::Event>>
where
    C: Decider,
    C::Event: Clone + Debug,
    C::EvolveError: Debug,
{
    /// Replays given events and provides the command under test.
    ///
    /// Panics when the given history cannot be replayed through [`Decider::evolve`].
    pub fn when(mut self, command: C) -> TestCase<C, When<C::Event, C::State, C>> {
        self.completed = true;
        let mut state = C::initial_state();
        let history = std::mem::take(&mut self.stage.history);

        for (index, event) in history.iter().enumerate() {
            state = match C::evolve(state, event) {
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
                expected_state: None,
                command,
            },
            completed: false,
        }
    }
}

impl<C> TestCase<C, When<C::Event, C::State, C>>
where
    C: Decider,
    C::Event: Clone + PartialEq + Debug,
    C::DecideError: PartialEq + Debug,
    C::EvolveError: Debug,
    C::StreamId: std::fmt::Display,
{
    /// Asserts the state rebuilt from the given history before the command is decided.
    pub fn state(mut self, expected_state: C::State) -> Self
    where
        C::State: PartialEq + Debug + 'static,
    {
        self.stage.expected_state = Some(Box::new(move |actual| {
            assert_eq!(
                actual, &expected_state,
                "state(...) replayed state did not match expectation"
            );
        }));
        self
    }

    /// Asserts the command emits the expected events.
    ///
    /// Accepts arrays, vectors, and [`Events`]. Event expectations must be non-empty.
    pub fn then<E>(self, expectation: E) -> E::Output
    where
        E: ThenExpectation<C>,
    {
        let When {
            history,
            state,
            expected_state,
            command,
        } = self.complete_into_stage();
        assert_expected_state(expected_state, &state);
        let actual = decide_events::<C>(state, &command);
        expectation.assert_matches(history, &command, actual)
    }

    /// Asserts the command returns the expected decision error.
    pub fn then_error(self, expected_error: C::DecideError) -> ThenError<C::Event, C::DecideError> {
        let When {
            history,
            state,
            expected_state,
            command,
        } = self.complete_into_stage();
        assert_expected_state(expected_state, &state);
        let actual = decide_events::<C>(state, &command);

        match actual {
            Err(DecisionEvaluationFailure::Decide(error)) => assert_eq!(
                error, expected_error,
                "then_error(...) expected an exact domain error, but the error did not match"
            ),
            Err(DecisionEvaluationFailure::Evolve(error)) => panic!(
                "then_error(...) expected a decide error, but emitted events could not be evolved:\nexpected error = {:?}\nactual evolve error = {:?}",
                expected_error, error,
            ),
            Ok(events) => panic!(
                "then_error(...) expected an error, but decide(...) emitted events:\nexpected error = {:?}\nactual events = {:?}",
                expected_error,
                events.as_slice(),
            ),
        }

        ThenError {
            stream_id: command.stream_id().to_string(),
            given: history,
            error: expected_error,
        }
    }
}

impl<C, Stage> Drop for TestCase<C, Stage> {
    fn drop(&mut self) {
        assert!(
            self.completed || std::thread::panicking(),
            "test cases must be completed with .then(...) or .then_error(...)"
        );
    }
}

/// Event expectation accepted by [`TestCase::then`].
pub trait ThenExpectation<C>: private::Sealed<C>
where
    C: Decider,
    C::StreamId: std::fmt::Display,
{
    type Output;

    fn assert_matches(self, given: History<C::Event>, command: &C, actual: DecisionEvaluation<C>) -> Self::Output;
}

impl<C, E> ThenExpectation<C> for Events<E>
where
    C: Decider,
    C::Event: Clone + PartialEq + Debug,
    C::DecideError: PartialEq + Debug,
    C::EvolveError: Debug,
    C::StreamId: std::fmt::Display,
    E: Into<C::Event>,
{
    type Output = ThenEvents<C::Event>;

    fn assert_matches(self, given: History<C::Event>, command: &C, actual: DecisionEvaluation<C>) -> Self::Output {
        assert_events_match::<C>(
            self.into_vec().into_iter().map(Into::into).collect(),
            given,
            command,
            actual,
        )
    }
}

impl<C> ThenExpectation<C> for Vec<C::Event>
where
    C: Decider,
    C::Event: Clone + PartialEq + Debug,
    C::DecideError: PartialEq + Debug,
    C::EvolveError: Debug,
    C::StreamId: std::fmt::Display,
{
    type Output = ThenEvents<C::Event>;

    fn assert_matches(self, given: History<C::Event>, command: &C, actual: DecisionEvaluation<C>) -> Self::Output {
        assert!(
            !self.is_empty(),
            "expected events in then(...), but event expectations must be non-empty"
        );
        assert_events_match::<C>(self, given, command, actual)
    }
}

impl<C, E, const N: usize> ThenExpectation<C> for [E; N]
where
    C: Decider,
    C::Event: Clone + PartialEq + Debug,
    C::DecideError: PartialEq + Debug,
    C::EvolveError: Debug,
    C::StreamId: std::fmt::Display,
    E: Into<C::Event>,
{
    type Output = ThenEvents<C::Event>;

    fn assert_matches(self, given: History<C::Event>, command: &C, actual: DecisionEvaluation<C>) -> Self::Output {
        assert!(
            N > 0,
            "expected events in then(...), but event expectations must be non-empty"
        );
        assert_events_match::<C>(
            Vec::from(self).into_iter().map(Into::into).collect(),
            given,
            command,
            actual,
        )
    }
}

fn assert_events_match<C>(
    expected: Vec<C::Event>,
    given: History<C::Event>,
    command: &C,
    actual: DecisionEvaluation<C>,
) -> ThenEvents<C::Event>
where
    C: Decider,
    C::Event: Clone + PartialEq + Debug,
    C::DecideError: PartialEq + Debug,
    C::EvolveError: Debug,
    C::StreamId: std::fmt::Display,
{
    match actual {
        Ok(events) => {
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
        Err(DecisionEvaluationFailure::Decide(error)) => panic!(
            "then(...) expected events, but decide(...) returned an error:\nexpected events = {:?}\nactual error = {:?}",
            expected, error,
        ),
        Err(DecisionEvaluationFailure::Evolve(error)) => panic!(
            "then(...) expected events, but emitted events could not be evolved:\nexpected events = {:?}\nactual evolve error = {:?}",
            expected, error,
        ),
    }
}

fn decide_events<C>(state: C::State, command: &C) -> DecisionEvaluation<C>
where
    C: Decider,
{
    let decision = C::decide(&state, command).map_err(DecisionEvaluationFailure::Decide)?;
    let (_, events) = decision.handle(state, command).map_err(|failure| match failure {
        DecisionFailure::Decide(error) => DecisionEvaluationFailure::Decide(error),
        DecisionFailure::Evolve(error) => DecisionEvaluationFailure::Evolve(error),
    })?;
    Ok(events)
}

fn assert_expected_state<State>(expected: Option<StateAssertion<State>>, actual: &State) {
    if let Some(expected) = expected {
        expected(actual);
    }
}

mod private {
    use crate::{Decider, Events};

    pub trait Sealed<C>
    where
        C: Decider,
    {
    }

    impl<C, E> Sealed<C> for Events<E>
    where
        C: Decider,
        E: Into<C::Event>,
    {
    }

    impl<C> Sealed<C> for Vec<C::Event> where C: Decider {}

    impl<C, E, const N: usize> Sealed<C> for [E; N]
    where
        C: Decider,
        E: Into<C::Event>,
    {
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;
    use crate::Decision;

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

    impl std::fmt::Display for TestDomainError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl std::error::Error for TestDomainError {}

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestCommandError {
        AlreadyRegistered { id: String },
        JobNotFound { id: String },
        AlreadyDisabled { id: String },
    }

    impl std::fmt::Display for TestCommandError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl std::error::Error for TestCommandError {}

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestAction {
        Register,
        Disable,
        Remove,
        EmitDisabled,
        EmitRegistered,
        ActReject,
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

        fn emit_disabled(id: &'static str) -> Self {
            Self {
                id,
                action: TestAction::EmitDisabled,
            }
        }

        fn emit_registered(id: &'static str) -> Self {
            Self {
                id,
                action: TestAction::EmitRegistered,
            }
        }

        fn act_reject(id: &'static str) -> Self {
            Self {
                id,
                action: TestAction::ActReject,
            }
        }
    }

    impl Decider for TestCommand {
        type StreamId = str;
        type State = TestState;
        type Event = TestEvent;
        type DecideError = TestCommandError;
        type EvolveError = TestDomainError;

        fn stream_id(&self) -> &Self::StreamId {
            self.id
        }

        fn initial_state() -> Self::State {
            TestState::Missing
        }

        fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
            match (state, event) {
                (TestState::Missing, TestEvent::Registered { .. }) => Ok(TestState::Present { enabled: true }),
                (TestState::Missing, TestEvent::Disabled { id }) => {
                    Err(TestDomainError::MissingJobForDisable { id: id.clone() })
                }
                (TestState::Missing, TestEvent::Removed { id }) => {
                    Err(TestDomainError::MissingJobForRemoval { id: id.clone() })
                }
                (TestState::Present { .. }, TestEvent::Registered { id }) => {
                    Err(TestDomainError::MissingJobForDisable { id: id.clone() })
                }
                (TestState::Present { .. }, TestEvent::Disabled { .. }) => Ok(TestState::Present { enabled: false }),
                (TestState::Present { .. }, TestEvent::Removed { .. }) => Ok(TestState::Missing),
            }
        }

        fn decide(state: &TestState, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
            match (&command.action, state) {
                (TestAction::Register, TestState::Missing) => Ok(Decision::event(TestEvent::Registered {
                    id: command.id.to_string(),
                })),
                (TestAction::Register, TestState::Present { .. }) => Err(TestCommandError::AlreadyRegistered {
                    id: command.id.to_string(),
                }),
                (TestAction::Disable, TestState::Missing) => Err(TestCommandError::JobNotFound {
                    id: command.id.to_string(),
                }),
                (TestAction::Disable, TestState::Present { enabled: false }) => {
                    Err(TestCommandError::AlreadyDisabled {
                        id: command.id.to_string(),
                    })
                }
                (TestAction::Disable, TestState::Present { enabled: true }) => {
                    Ok(Decision::event(TestEvent::Disabled {
                        id: command.id.to_string(),
                    }))
                }
                (TestAction::Remove, TestState::Missing) => Err(TestCommandError::JobNotFound {
                    id: command.id.to_string(),
                }),
                (TestAction::Remove, TestState::Present { .. }) => Ok(Decision::event(TestEvent::Removed {
                    id: command.id.to_string(),
                })),
                (TestAction::EmitDisabled, _) => Ok(Decision::event(TestEvent::Disabled {
                    id: command.id.to_string(),
                })),
                (TestAction::EmitRegistered, _) => Ok(Decision::event(TestEvent::Registered {
                    id: command.id.to_string(),
                })),
                (TestAction::ActReject, _) => Decision::act()
                    .execute(|_: &TestState, command: &TestCommand| {
                        Err(TestCommandError::JobNotFound {
                            id: command.id.to_string(),
                        })
                    })
                    .into(),
            }
        }
    }

    #[test]
    fn given_empty_history_when_command_then_array_events_succeeds() {
        let register = TestCase::<TestCommand>::new()
            .given_no_history()
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
        TestCase::<TestCommand>::new()
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
        TestCase::<TestCommand>::new()
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::register("alpha"))
            .then_error(TestCommandError::AlreadyRegistered {
                id: "alpha".to_string(),
            });
    }

    #[test]
    fn state_can_assert_replayed_state_before_events() {
        TestCase::<TestCommand>::new()
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::disable("alpha"))
            .state(TestState::Present { enabled: true })
            .then([TestEvent::Disabled {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn state_can_assert_replayed_state_before_errors() {
        TestCase::<TestCommand>::new()
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .given([TestEvent::Disabled {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::disable("alpha"))
            .state(TestState::Present { enabled: false })
            .then_error(TestCommandError::AlreadyDisabled {
                id: "alpha".to_string(),
            });
    }

    #[test]
    #[should_panic(expected = "Given history could not be replayed at event 1")]
    fn invalid_history_in_given_panics_during_replay() {
        let _ = TestCase::<TestCommand>::new()
            .given([TestEvent::Removed {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::register("alpha"));
    }

    #[test]
    fn then_vec_events_works() {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then(vec![TestEvent::Registered {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn then_events_works() {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then(Events::one(TestEvent::Registered {
                id: "alpha".to_string(),
            }));
    }

    #[test]
    fn then_error_works() {
        let error = TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::remove("alpha"))
            .then_error(TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            });

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
    fn then_error_works_for_disable_without_history() {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::disable("alpha"))
            .then_error(TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            });
    }

    #[test]
    fn test_errors_display_debug_shape() {
        assert_eq!(
            TestDomainError::MissingJobForDisable {
                id: "alpha".to_string()
            }
            .to_string(),
            r#"MissingJobForDisable { id: "alpha" }"#
        );
        assert_eq!(
            TestCommandError::AlreadyRegistered {
                id: "alpha".to_string()
            }
            .to_string(),
            r#"AlreadyRegistered { id: "alpha" }"#
        );
    }

    #[test]
    fn history_combines_given_and_emitted_events() {
        let register = TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);

        let disable = TestCase::<TestCommand>::new()
            .given(register.history())
            .when(TestCommand::disable("alpha"))
            .then([TestEvent::Disabled {
                id: "alpha".to_string(),
            }]);

        assert_eq!(
            disable.history().as_slice(),
            [
                TestEvent::Registered {
                    id: "alpha".to_string(),
                },
                TestEvent::Disabled {
                    id: "alpha".to_string(),
                },
            ]
        );
    }

    #[test]
    #[should_panic(expected = "event expectations must be non-empty")]
    fn then_empty_event_vec_panics() {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then(Vec::<TestEvent>::new());
    }

    #[test]
    fn multiple_given_calls_append_to_history() {
        TestCase::<TestCommand>::new()
            .given([TestEvent::Registered {
                id: "alpha".to_string(),
            }])
            .given([TestEvent::Disabled {
                id: "alpha".to_string(),
            }])
            .when(TestCommand::remove("alpha"))
            .then([TestEvent::Removed {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn history_conversions_and_accessors_work() {
        let empty = History::<TestEvent>::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let from_vec = History::from(vec![TestEvent::Registered {
            id: "alpha".to_string(),
        }]);
        assert_eq!(from_vec.as_ref(), from_vec.as_slice());
        assert_eq!(from_vec.iter().count(), 1);

        let from_array = History::from([TestEvent::Disabled {
            id: "alpha".to_string(),
        }]);
        assert_eq!(from_array.into_vec().len(), 1);

        let collected = [
            TestEvent::Registered {
                id: "alpha".to_string(),
            },
            TestEvent::Disabled {
                id: "alpha".to_string(),
            },
        ]
        .into_iter()
        .collect::<History<_>>();
        assert_eq!(collected.into_iter().count(), 2);
    }

    #[test]
    fn default_test_case_starts_a_valid_chain() {
        TestCase::<TestCommand>::default()
            .given_no_history()
            .when(TestCommand::register("alpha"))
            .then([TestEvent::Registered {
                id: "alpha".to_string(),
            }]);
    }

    #[test]
    fn unfinished_test_case_panics_on_drop() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _case = TestCase::<TestCommand>::new().given_no_history();
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("test cases must be completed with .then(...) or .then_error(...)"));
    }

    #[test]
    fn event_mismatch_shows_expected_vs_actual() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::<TestCommand>::new()
                .given_no_history()
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
    fn event_expectation_panics_when_decide_rejects() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::<TestCommand>::new()
                .given_no_history()
                .when(TestCommand::remove("alpha"))
                .then([TestEvent::Removed {
                    id: "alpha".to_string(),
                }]);
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("then(...) expected events"));
        assert!(message.contains("JobNotFound"));
    }

    #[test]
    fn event_expectation_panics_when_emitted_events_do_not_evolve() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::<TestCommand>::new()
                .given_no_history()
                .when(TestCommand::emit_disabled("alpha"))
                .then([TestEvent::Disabled {
                    id: "alpha".to_string(),
                }]);
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("emitted events could not be evolved"));
        assert!(message.contains("MissingJobForDisable"));
    }

    #[test]
    fn error_expectation_panics_when_emitted_events_do_not_evolve() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::<TestCommand>::new()
                .given_no_history()
                .when(TestCommand::emit_disabled("alpha"))
                .then_error(TestCommandError::AlreadyDisabled {
                    id: "alpha".to_string(),
                });
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("then_error(...) expected a decide error"));
        assert!(message.contains("MissingJobForDisable"));
    }

    #[test]
    fn act_decide_failure_is_reported_as_decide_error() {
        TestCase::<TestCommand>::new()
            .given_no_history()
            .when(TestCommand::act_reject("alpha"))
            .then_error(TestCommandError::JobNotFound {
                id: "alpha".to_string(),
            });
    }

    #[test]
    fn invalid_emitted_event_after_history_reports_evolve_error() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::<TestCommand>::new()
                .given([TestEvent::Registered {
                    id: "alpha".to_string(),
                }])
                .when(TestCommand::emit_registered("alpha"))
                .then([TestEvent::Registered {
                    id: "alpha".to_string(),
                }]);
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("MissingJobForDisable"));
    }

    #[test]
    fn error_mismatch_shows_expected_vs_actual() {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            TestCase::<TestCommand>::new()
                .given_no_history()
                .when(TestCommand::register("alpha"))
                .then_error(TestCommandError::JobNotFound {
                    id: "alpha".to_string(),
                });
        }))
        .unwrap_err();

        let message = panic_message(&panic);
        assert!(message.contains("then_error(...) expected an error"));
        assert!(message.contains("JobNotFound"));
        assert!(message.contains("Registered"));
    }

    #[test]
    fn panic_message_returns_empty_for_unknown_payload() {
        let panic = catch_unwind(AssertUnwindSafe(|| std::panic::panic_any(123_u8))).unwrap_err();

        assert_eq!(panic_message(&panic), "");
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
