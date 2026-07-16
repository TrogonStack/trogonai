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
//! #[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
//! enum AccountError {
//!     #[error("{self:?}")]
//!     AlreadyOpen,
//! }
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
//! # #[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
//! # enum AccountError {
//! #     #[error("{self:?}")]
//! #     AlreadyOpen,
//! # }
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

use crate::{Decider, DecisionFailure, Events, evaluate_decision};

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
/// The value records the stream id, prior history, events emitted by one completed
/// [`TestCase`], and the state produced by replaying those events. Feed
/// [`history`](Self::history) into the next case when a test needs to model multiple
/// commands against the same stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThenEvents<Event, State> {
    stream_id: String,
    given: History<Event>,
    then: Events<Event>,
    resulting_state: State,
}

impl<Event, State> ThenEvents<Event, State> {
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

    /// State produced by replaying [`then_events`](Self::then_events) on top of the
    /// state the command decided against.
    ///
    /// This is the post-decision state: `decide` followed by `evolve` over every
    /// emitted event. Use [`then_state`](Self::then_state) to assert it inline, or
    /// this accessor to inspect it directly.
    pub fn resulting_state(&self) -> &State {
        &self.resulting_state
    }
}

impl<Event, State> ThenEvents<Event, State>
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

impl<Event, State> ThenEvents<Event, State>
where
    State: PartialEq + Debug,
{
    /// Asserts the state produced after this command's events are replayed.
    ///
    /// # Example
    ///
    /// ```
    /// # use trogon_decider::{Decider, Decision};
    /// # use trogon_decider::testing::TestCase;
    /// #
    /// # #[derive(Debug, Clone, PartialEq, Eq)]
    /// # enum AccountState {
    /// #     Missing,
    /// #     Open,
    /// # }
    /// #
    /// # #[derive(Debug, Clone, PartialEq, Eq)]
    /// # enum AccountEvent {
    /// #     Opened,
    /// # }
    /// #
    /// # #[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
    /// # enum AccountError {
    /// #     #[error("{self:?}")]
    /// #     AlreadyOpen,
    /// # }
    /// #
    /// # #[derive(Debug, Clone, PartialEq, Eq)]
    /// # struct OpenAccount {
    /// #     account_id: &'static str,
    /// # }
    /// #
    /// # impl Decider for OpenAccount {
    /// #     type StreamId = str;
    /// #     type State = AccountState;
    /// #     type Event = AccountEvent;
    /// #     type DecideError = AccountError;
    /// #     type EvolveError = AccountError;
    /// #
    /// #     fn stream_id(&self) -> &Self::StreamId { self.account_id }
    /// #     fn initial_state() -> Self::State { AccountState::Missing }
    /// #     fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
    /// #         match event {
    /// #             AccountEvent::Opened => Ok(AccountState::Open),
    /// #         }
    /// #     }
    /// #     fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
    /// #         match state {
    /// #             AccountState::Missing => Ok(Decision::event(AccountEvent::Opened)),
    /// #             AccountState::Open => Err(AccountError::AlreadyOpen),
    /// #         }
    /// #     }
    /// # }
    /// #
    /// let opened = TestCase::<OpenAccount>::new()
    ///     .given_no_history()
    ///     .when(OpenAccount { account_id: "account-1" })
    ///     .then([AccountEvent::Opened])
    ///     .then_state(AccountState::Open);
    ///
    /// assert_eq!(opened.resulting_state(), &AccountState::Open);
    /// ```
    pub fn then_state(self, expected: State) -> Self {
        assert_eq!(
            self.resulting_state, expected,
            "then_state(...) resulting state did not match expectation"
        );
        self
    }
}

/// A completed error expectation.
///
/// The value records the stream id, prior history, decision error, and stable wire
/// code (from [`Decider::decide_error_code`]) of one completed [`TestCase`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThenError<Event, Error> {
    stream_id: String,
    given: History<Event>,
    error: Error,
    code: String,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionEvaluationFailure<DecideError, EvolveError> {
    Decide(DecideError),
    Evolve(EvolveError),
}

#[doc(hidden)]
pub type DecisionEvaluation<C> = Result<
    (Events<<C as Decider>::Event>, <C as Decider>::State),
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

    /// Stable wire code for [`error`](Self::error), from [`Decider::decide_error_code`].
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Asserts the decision error's stable wire code from [`Decider::decide_error_code`].
    ///
    /// [`TestCase::then_error`] already pins the exact `DecideError` variant; this
    /// combinator additionally pins the string code the WASM bridge surfaces for that
    /// variant, so a code rename (without a matching variant rename) fails the test.
    ///
    /// # Example
    ///
    /// ```
    /// # use trogon_decider::{Decider, Decision};
    /// # use trogon_decider::testing::TestCase;
    /// #
    /// # #[derive(Debug, Clone, PartialEq, Eq)]
    /// # enum AccountState {
    /// #     Missing,
    /// #     Open,
    /// # }
    /// #
    /// # #[derive(Debug, Clone, PartialEq, Eq)]
    /// # enum AccountEvent {
    /// #     Opened,
    /// # }
    /// #
    /// # #[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
    /// # enum AccountError {
    /// #     #[error("{self:?}")]
    /// #     AlreadyOpen,
    /// # }
    /// #
    /// # #[derive(Debug, Clone, PartialEq, Eq)]
    /// # struct OpenAccount {
    /// #     account_id: &'static str,
    /// # }
    /// #
    /// # impl Decider for OpenAccount {
    /// #     type StreamId = str;
    /// #     type State = AccountState;
    /// #     type Event = AccountEvent;
    /// #     type DecideError = AccountError;
    /// #     type EvolveError = AccountError;
    /// #
    /// #     fn stream_id(&self) -> &Self::StreamId { self.account_id }
    /// #     fn initial_state() -> Self::State { AccountState::Missing }
    /// #     fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
    /// #         match event {
    /// #             AccountEvent::Opened => Ok(AccountState::Open),
    /// #         }
    /// #     }
    /// #     fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
    /// #         match state {
    /// #             AccountState::Missing => Ok(Decision::event(AccountEvent::Opened)),
    /// #             AccountState::Open => Err(AccountError::AlreadyOpen),
    /// #         }
    /// #     }
    /// #     fn decide_error_code(error: &Self::DecideError) -> &str {
    /// #         match error {
    /// #             AccountError::AlreadyOpen => "already-open",
    /// #         }
    /// #     }
    /// # }
    /// #
    /// TestCase::<OpenAccount>::new()
    ///     .given([AccountEvent::Opened])
    ///     .when(OpenAccount { account_id: "account-1" })
    ///     .then_error(AccountError::AlreadyOpen)
    ///     .then_error_code("already-open");
    /// ```
    pub fn then_error_code(self, expected: &str) -> Self {
        assert_eq!(
            self.code, expected,
            "then_error_code(...) expected decide_error_code {expected:?}, but got {:?}",
            self.code,
        );
        self
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
pub struct GivenState<State> {
    state: State,
}

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

    /// Starts from an explicit prebuilt state instead of replayed events.
    ///
    /// Reserved for decide guards that defend against states no event history
    /// can produce, such as corrupted persisted protobuf or a degenerate
    /// sequence value. Prefer [`given`](Self::given) or
    /// [`given_no_history`](Self::given_no_history) for any behavior reachable
    /// through real events.
    pub fn given_state(mut self, state: C::State) -> TestCase<C, GivenState<C::State>> {
        self.completed = true;
        TestCase {
            marker: PhantomData,
            stage: GivenState { state },
            completed: false,
        }
    }
}

impl<C> TestCase<C, GivenState<C::State>>
where
    C: Decider,
{
    /// Provides the command under test against the seeded state.
    pub fn when(self, command: C) -> TestCase<C, When<C::Event, C::State, C>> {
        let GivenState { state } = self.complete_into_stage();
        TestCase {
            marker: PhantomData,
            stage: When {
                history: History::new(),
                state,
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
    #[allow(
        clippy::disallowed_methods,
        reason = "TestCase is the sanctioned harness; it replays history through evolve so tests don't call it directly"
    )]
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
            Ok((events, _state)) => panic!(
                "then_error(...) expected an error, but decide(...) emitted events:\nexpected error = {:?}\nactual events = {:?}",
                expected_error,
                events.as_slice(),
            ),
        }

        let code = C::decide_error_code(&expected_error).to_string();

        ThenError {
            stream_id: command.stream_id().to_string(),
            given: history,
            error: expected_error,
            code,
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
    type Output = ThenEvents<C::Event, C::State>;

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
    type Output = ThenEvents<C::Event, C::State>;

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
    type Output = ThenEvents<C::Event, C::State>;

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
) -> ThenEvents<C::Event, C::State>
where
    C: Decider,
    C::Event: Clone + PartialEq + Debug,
    C::DecideError: PartialEq + Debug,
    C::EvolveError: Debug,
    C::StreamId: std::fmt::Display,
{
    match actual {
        Ok((events, resulting_state)) => {
            assert_eq!(
                events.as_slice(),
                expected.as_slice(),
                "then(...) emitted events did not match expectation"
            );
            ThenEvents {
                stream_id: command.stream_id().to_string(),
                given,
                then: events,
                resulting_state,
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
    let (state, events) = evaluate_decision(state, command).map_err(|failure| match failure {
        DecisionFailure::Decide(error) => DecisionEvaluationFailure::Decide(error),
        DecisionFailure::Evolve(error) => DecisionEvaluationFailure::Evolve(error),
    })?;
    Ok((events, state))
}

fn assert_expected_state<State>(expected: Option<StateAssertion<State>>, actual: &State) {
    if let Some(expected) = expected {
        expected(actual);
    }
}

mod codec;
mod private;

pub use codec::assert_round_trips;

#[cfg(test)]
mod tests;
