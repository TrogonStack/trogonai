#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
//! Kill switch as a decider aggregate.
//!
//! Models an agent's kill/alive lifecycle as a [`trogon_decider::Decider`]
//! pair ([`Kill`], [`Revive`]) over one JetStream subject per agent, the same
//! shape as `trogon-scheduler-domain`'s schedule commands. `decide` and
//! `evolve` are pure and infallible-on-evolve (see [`KillSwitchEvent`]); no
//! I/O, no enforcement, and no policy about *who* may call [`Kill`] or
//! [`Revive`] lives here; that belongs to the caller and, ultimately, to a
//! NATS subject-permission enforcement layer described in
//! `docs/proposals/kill-switch-enforcement.md`.
//!
//! Enable the `nats-projection` feature for a JetStream Key/Value projection
//! of current kill state, modeled on `a2a-nats`'s `KvCatalogStore`.

mod agent_id;
mod commands;
mod event;
mod event_wire;
mod kill_reason;
mod occurred_at;
mod state;

#[cfg(feature = "nats-projection")]
pub mod projection;

pub use agent_id::{AgentId, AgentIdError, AgentIdViolation};
pub use commands::{Kill, KillError, Revive, ReviveError};
pub use event::KillSwitchEvent;
pub use event_wire::KillSwitchEventCodecError;
pub use kill_reason::{KillReason, KillReasonError};
pub use occurred_at::{OccurredAt, OccurredAtError};
pub use state::KillSwitchState;
