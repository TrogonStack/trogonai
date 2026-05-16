#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
//! Typed decider primitives for event-sourced workflows.
//!
//! A *decider* turns a command into events against a prior state. This crate provides:
//!
//! - The [`Decider`] trait, which ties together a command, its [`State`](Decider::State),
//!   [`Event`](Decider::Event), and the rules for replaying events ([`evolve`](Decider::evolve))
//!   and producing new ones ([`decide`](Decider::decide)).
//! - [`Decision`], the outcome of `decide`: either a batch of events directly, or a
//!   multi-step plan ([`Decision::act`]) whose later steps observe the state that would
//!   result from earlier steps.
//! - [`Events`], a non-empty ordered batch, and [`WritePrecondition`], an optional
//!   concurrency guard applied when persisting.
//!
//! # Example
//!
//! ```
//! use trogon_decider::{Decider, Decision, WritePrecondition};
//!
//! struct PlaceOrder {
//!     order_id: String,
//!     customer_id: String,
//! }
//!
//! #[derive(Debug, PartialEq, Eq)]
//! enum OrderEvent {
//!     Placed { order_id: String, customer_id: String },
//! }
//!
//! #[derive(Debug, PartialEq, Eq)]
//! enum OrderState {
//!     New,
//!     Placed { customer_id: String },
//! }
//!
//! #[derive(Debug, PartialEq, Eq)]
//! enum PlaceOrderError {
//!     AlreadyPlaced,
//! }
//!
//! impl std::fmt::Display for PlaceOrderError {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         match self {
//!             Self::AlreadyPlaced => f.write_str("order is already placed"),
//!         }
//!     }
//! }
//!
//! impl std::error::Error for PlaceOrderError {}
//!
//! #[derive(Debug, PartialEq, Eq)]
//! struct PlaceOrderEvolveError;
//!
//! impl std::fmt::Display for PlaceOrderEvolveError {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         f.write_str("invalid order event")
//!     }
//! }
//!
//! impl std::error::Error for PlaceOrderEvolveError {}
//!
//! impl Decider for PlaceOrder {
//!     type StreamId = str;
//!     type State = OrderState;
//!     type Event = OrderEvent;
//!     type DecideError = PlaceOrderError;
//!     type EvolveError = PlaceOrderEvolveError;
//!
//!     const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);
//!
//!     fn stream_id(&self) -> &Self::StreamId { &self.order_id }
//!
//!     fn initial_state() -> Self::State { OrderState::New }
//!
//!     fn evolve(_state: Self::State, event: &Self::Event) -> Result<Self::State, PlaceOrderEvolveError> {
//!         match event {
//!             OrderEvent::Placed { customer_id, .. } => Ok(OrderState::Placed {
//!                 customer_id: customer_id.clone(),
//!             }),
//!         }
//!     }
//!
//!     fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, PlaceOrderError> {
//!         match state {
//!             OrderState::Placed { .. } => Err(PlaceOrderError::AlreadyPlaced),
//!             OrderState::New => Ok(Decision::event(OrderEvent::Placed {
//!                 order_id: command.order_id.clone(),
//!                 customer_id: command.customer_id.clone(),
//!             })),
//!         }
//!     }
//! }
//!
//! let command = PlaceOrder {
//!     order_id: "order-42".into(),
//!     customer_id: "alice".into(),
//! };
//! let state = PlaceOrder::initial_state();
//! let decision = PlaceOrder::decide(&state, &command);
//! assert!(decision.is_ok());
//! ```

mod act;
mod decision;
mod events;
#[cfg(feature = "test-support")]
pub mod testing;
mod write_precondition;

pub use act::{Act, ActBuilder};
#[doc(hidden)]
pub use act::{ActChain, ActRun, First, Steps, Then};
pub use decision::Decision;
#[doc(hidden)]
pub use decision::{DecisionFailure, DecisionResult};
pub use events::Events;
#[cfg(feature = "test-support")]
pub use testing::{History, TestCase, ThenError, ThenEvents, ThenExpectation};
pub use write_precondition::WritePrecondition;

/// Decision-making for a typed command.
///
/// An implementation replays prior events through [`evolve`](Self::evolve) to rebuild
/// [`State`](Self::State), then evaluates the command against that state in
/// [`decide`](Self::decide) to produce a [`Decision`].
///
/// The trait is `Sized` so `&Self` and `&Self::State` can be passed around without
/// erasure. Errors are split into two channels: [`DecideError`](Self::DecideError) for
/// rule-level rejections in `decide`, and [`EvolveError`](Self::EvolveError) for invalid
/// event payloads encountered in `evolve`.
pub trait Decider: Sized {
    /// Stream identifier type. Usually `str` so commands return a borrowed slice.
    type StreamId: ?Sized;

    /// State rebuilt from prior events.
    type State;

    /// Domain event appended to the stream.
    type Event;

    /// Error returned when a command cannot be turned into events.
    type DecideError: std::error::Error;

    /// Error returned when an event cannot be applied to state.
    type EvolveError: std::error::Error;

    /// Optional concurrency guard applied when persisting the resulting events.
    ///
    /// Defaults to `None` (no precondition). Override per command when you need to
    /// enforce e.g. "stream must not exist" for a creation command.
    const WRITE_PRECONDITION: Option<WritePrecondition> = None;

    /// Returns the stream identifier this command targets.
    fn stream_id(&self) -> &Self::StreamId;

    /// Returns the state used before any events have been replayed.
    fn initial_state() -> Self::State;

    /// Applies one event to the current state, yielding the new state.
    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError>;

    /// Decides which events the command produces against the current state.
    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct TestCommand;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestError {
        Decide,
        Evolve,
    }

    impl std::fmt::Display for TestError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Decide => formatter.write_str("test decide error"),
                Self::Evolve => formatter.write_str("test evolve error"),
            }
        }
    }

    impl std::error::Error for TestError {}

    impl Decider for TestCommand {
        type StreamId = str;
        type State = u8;
        type Event = &'static str;
        type DecideError = TestError;
        type EvolveError = TestError;

        fn stream_id(&self) -> &Self::StreamId {
            "alpha"
        }

        fn initial_state() -> Self::State {
            0
        }

        fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
            if *event == "broken" {
                Err(TestError::Evolve)
            } else {
                Ok(state + 1)
            }
        }

        fn decide(state: &Self::State, _command: &Self) -> Result<Decision<Self>, Self::DecideError> {
            Ok(Decision::event(if *state == 1 { "created" } else { "updated" }))
        }
    }

    #[test]
    fn act_chains_plain_decisions() {
        let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
            .execute(|_, _| Decision::event("created"))
            .execute(|_, _| Decision::event("updated"))
            .into();

        let (state, events) = decision.unwrap().handle(0, &TestCommand).unwrap();

        assert_eq!(state, 2);
        assert_eq!(events.as_slice(), &["created", "updated"]);
    }

    #[test]
    fn act_accepts_result_steps() {
        let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
            .execute(|_, _| Ok(Decision::event("created")))
            .into();

        let (state, events) = decision.unwrap().handle(0, &TestCommand).unwrap();

        assert_eq!(state, 1);
        assert_eq!(events.as_slice(), &["created"]);
    }

    #[test]
    fn act_steps_see_evolved_state() {
        let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
            .execute(|_, _| Decision::event("created"))
            .execute(|state, _| Decision::event(if *state == 1 { "updated" } else { "broken" }))
            .into();

        let (state, events) = decision.unwrap().handle(0, &TestCommand).unwrap();

        assert_eq!(state, 2);
        assert_eq!(events.as_slice(), &["created", "updated"]);
    }

    #[test]
    fn act_propagates_decide_errors() {
        let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
            .execute(|_, _| Err(TestError::Decide))
            .into();

        assert_eq!(
            decision.unwrap().handle(0, &TestCommand),
            Err(DecisionFailure::Decide(TestError::Decide))
        );
    }

    #[test]
    fn act_propagates_evolve_errors() {
        let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
            .execute(|_, _| Decision::event("broken"))
            .into();

        assert_eq!(
            decision.unwrap().handle(0, &TestCommand),
            Err(DecisionFailure::Evolve(TestError::Evolve))
        );
    }

    #[test]
    fn decide_exposes_typed_stream_id() {
        let command = TestCommand;
        assert_eq!(command.stream_id(), "alpha");
    }

    #[test]
    fn decider_trait_exposes_initial_state_and_decision() {
        let command = TestCommand;
        let decision = TestCommand::decide(&TestCommand::initial_state(), &command).unwrap();
        let (state, events) = decision.handle(TestCommand::initial_state(), &command).unwrap();

        assert_eq!(state, 1);
        assert_eq!(events.as_slice(), &["updated"]);
    }

    #[test]
    fn events_rejects_empty_vectors() {
        assert!(Events::<u8>::from_vec(vec![]).is_none());
        assert_eq!(Events::from_vec(vec![1, 2]).unwrap().as_slice(), &[1, 2]);
    }

    #[test]
    fn events_supports_map_and_iteration() {
        let events = Events::from_vec(vec![1, 2, 3]).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(*events.first(), 1);
        assert!(!events.is_empty());
        assert_eq!(events.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(events.map(|value| value.to_string()).into_vec(), vec!["1", "2", "3"]);
    }

    #[test]
    fn events_supports_guaranteed_first_and_fallible_map() {
        let events = Events::from_first(1, vec![2, 3]);
        let mapped: Result<Events<_>, TestError> = events.clone().try_map(|value| Ok(value.to_string()));
        let failed: Result<Events<_>, TestError> =
            events.try_map(|value| if value == 2 { Err(TestError::Evolve) } else { Ok(value) });

        assert_eq!(mapped.unwrap().into_vec(), vec!["1", "2", "3"]);
        assert_eq!(failed, Err(TestError::Evolve));
    }

    #[test]
    fn decision_event_evolves_state_through_handle() {
        let decision = Decision::<TestCommand>::event("created");
        let (state, events) = decision.handle(0, &TestCommand).unwrap();
        assert_eq!(state, 1);
        assert_eq!(events.as_slice(), &["created"]);
    }

    #[test]
    fn decision_events_evolves_each_event_through_handle() {
        let payload = Events::from_vec(vec!["a", "b", "c"]).unwrap();
        let (state, events) = Decision::<TestCommand>::events(payload)
            .handle(0, &TestCommand)
            .unwrap();
        assert_eq!(state, 3);
        assert_eq!(events.as_slice(), &["a", "b", "c"]);
    }

    #[test]
    fn decision_events_direct_path_propagates_evolve_errors() {
        let decision = Decision::<TestCommand>::event("broken");
        assert_eq!(
            decision.handle(0, &TestCommand),
            Err(DecisionFailure::Evolve(TestError::Evolve))
        );
    }

    #[test]
    fn decision_failure_exposes_display_and_source() {
        let decide = DecisionFailure::<TestError, TestError>::Decide(TestError::Decide);
        let evolve = DecisionFailure::<TestError, TestError>::Evolve(TestError::Evolve);

        assert_eq!(decide.to_string(), "decide failed: test decide error");
        assert_eq!(evolve.to_string(), "evolve failed: test evolve error");
        assert!(std::error::Error::source(&decide).is_some());
        assert!(std::error::Error::source(&evolve).is_some());
    }

    #[test]
    fn act_chains_three_steps_threads_state() {
        let decision: Result<Decision<TestCommand>, _> = Decision::<TestCommand>::act()
            .execute(|state, _| Decision::event(if *state == 0 { "a" } else { "broken" }))
            .execute(|state, _| Decision::event(if *state == 1 { "b" } else { "broken" }))
            .execute(|state, _| Decision::event(if *state == 2 { "c" } else { "broken" }))
            .into();

        let (state, events) = decision.unwrap().handle(0, &TestCommand).unwrap();
        assert_eq!(state, 3);
        assert_eq!(events.as_slice(), &["a", "b", "c"]);
    }

    #[test]
    fn from_decision_to_result_wraps_in_ok() {
        let decision = Decision::<TestCommand>::event("created");
        let result: Result<Decision<TestCommand>, TestError> = decision.into();
        let (state, events) = result.unwrap().handle(0, &TestCommand).unwrap();
        assert_eq!(state, 1);
        assert_eq!(events.as_slice(), &["created"]);
    }

    #[test]
    fn act_types_expose_debug_and_default() {
        type Step = fn(&u8, &TestCommand) -> Decision<TestCommand>;

        let step: Step = |_, _| Decision::event("created");
        let chain = ActBuilder::<TestCommand>::default().execute(step);
        assert!(format!("{chain:?}").contains("ActChain"));

        let chain = chain.execute(step);
        let decision: Result<Decision<TestCommand>, TestError> = chain.into();
        let decision = decision.unwrap();

        assert!(format!("{:?}", ActBuilder::<TestCommand>::default()).contains("ActBuilder"));
        assert!(format!("{decision:?}").contains("Act"));
    }
}
