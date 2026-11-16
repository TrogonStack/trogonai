#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        acyclic_modules,
        reason = "`Decision::Act` holds an `Act` whose steps each produce a `Decision`, so the two types are mutually recursive"
    )
)]
//! Typed decider primitives for event-sourced workflows.
//!
//! A *decider* turns a command into events against a prior state. This crate provides:
//!
//! - The [`Decider`] trait, which ties together a command, its [`State`](Decider::State),
//!   [`Event`](Decider::Event), and the rules for producing new events.
//! - The [`Evolve`] trait, which lets each decider's state replay prior events.
//! - [`Decision`], the outcome of `decide`: either a batch of events directly, or a
//!   multi-step plan ([`Decision::act`]) whose later steps observe the state that would
//!   result from earlier steps.
//! - [`Events`], a non-empty ordered batch, [`WritePrecondition`], the concurrency guard a
//!   command declares for persisting its events, and [`SnapshotCadence`], how often it asks for
//!   its state to be snapshotted.
//!
//! # Example
//!
//! ```
//! use trogon_decider::{Decider, Decision, Evolve, WritePrecondition};
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
//! #[derive(thiserror::Error, Debug, PartialEq, Eq)]
//! enum PlaceOrderError {
//!     #[error("order is already placed")]
//!     AlreadyPlaced,
//! }
//!
//! #[derive(thiserror::Error, Debug, PartialEq, Eq)]
//! #[error("invalid order event")]
//! struct PlaceOrderEvolveError;
//!
//! impl Default for OrderState {
//!     fn default() -> Self { Self::New }
//! }
//!
//! impl Evolve<OrderEvent> for OrderState {
//!     type Error = PlaceOrderEvolveError;
//!
//!     fn evolve(self, event: &OrderEvent) -> Result<Self, Self::Error> {
//!         match event {
//!             OrderEvent::Placed { customer_id, .. } => Ok(Self::Placed {
//!                 customer_id: customer_id.clone(),
//!             }),
//!         }
//!     }
//! }
//!
//! impl Decider for PlaceOrder {
//!     type StreamId = str;
//!     type State = OrderState;
//!     type Event = OrderEvent;
//!     type DecideError = PlaceOrderError;
//!
//!     const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::NoStream;
//!
//!     fn stream_id(&self) -> &Self::StreamId { &self.order_id }
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
//! let state = OrderState::default();
//! let decision = PlaceOrder::decide(&state, &command);
//! assert!(decision.is_ok());
//! ```

mod act;
mod decider_state;
mod decision;
pub mod event;
mod events;
mod snapshot_cadence;
mod state_snapshot;
#[cfg(feature = "test-support")]
pub mod testing;
mod write_precondition;

pub use act::{Act, ActBuilder};
#[doc(hidden)]
pub use act::{ActChain, ActRun, First, Steps, Then};
pub use decider_state::DeciderState;
pub use decision::Decision;
#[doc(hidden)]
pub use decision::{DecisionError, DecisionResult, evaluate_decision};
pub use event::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventPayloadError, EventType};
pub use events::Events;
pub use snapshot_cadence::SnapshotCadence;
pub use state_snapshot::StateSnapshot;
#[cfg(feature = "test-support")]
pub use testing::{History, TestCase, ThenError, ThenEvents, ThenExpectation};
pub use write_precondition::WritePrecondition;

/// Replays an event into the state selected by a decider.
pub trait Evolve<Event>: Sized {
    /// Error returned when an event cannot be applied to state.
    type Error: std::error::Error;

    /// Applies one event to the current state, yielding the new state.
    fn evolve(self, event: &Event) -> Result<Self, Self::Error>;
}

/// Replay error selected by a decider's state type.
pub type EvolveError<C> = <<C as Decider>::State as Evolve<<C as Decider>::Event>>::Error;

/// Decision-making for a typed command.
///
/// [`DeciderState`] combines the state's no-history value through [`Default`] and event replay
/// through [`Evolve`]. The command only decides which new events to produce.
pub trait Decider: Sized {
    /// Stream identifier type. Usually `str` so commands return a borrowed slice.
    type StreamId: ?Sized;

    /// State rebuilt from prior events.
    type State: DeciderState<Self::Event>;

    /// Domain event appended to the stream.
    type Event;

    /// Error returned when a command cannot be turned into events.
    type DecideError: std::error::Error;

    /// Concurrency guard applied when persisting the resulting events.
    ///
    /// Required, with no default, so every command states what its meaning needs at the point it
    /// is defined. [`WritePrecondition::StreamUnchanged`] is the optimistic-concurrency choice for
    /// an ordinary state transition; [`WritePrecondition::NoStream`] for a creation command.
    const WRITE_PRECONDITION: WritePrecondition;

    /// Snapshot cadence applied after this command's events are appended.
    ///
    /// Defaults to [`SnapshotCadence::Never`]: unlike the write precondition, an omitted cadence
    /// costs replay time rather than correctness. Declared here, at the command, so the native and
    /// sandboxed execution paths cannot drift apart on it.
    const SNAPSHOT_CADENCE: SnapshotCadence = SnapshotCadence::Never;

    /// Returns the stream identifier this command targets.
    fn stream_id(&self) -> &Self::StreamId;

    /// Decides which events the command produces against the current state.
    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError>;
}

#[cfg(test)]
mod tests;
