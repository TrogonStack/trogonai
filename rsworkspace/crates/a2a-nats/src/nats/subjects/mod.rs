//! Subject typestate marker traits + per-operation subject modules.
//!
//! Each subject type implements one of [`markers::Requestable`],
//! [`markers::Publishable`], [`markers::Subscribable`], or [`markers::JetStreamEvents`]
//! so call-sites can't accidentally `request()` a fire-and-forget subject. Per-operation
//! subject types (`MessageSendSubject`, `TaskEventsSubject`, …) land in their dedicated
//! PRs under [`agents`], [`tasks`], and [`subscriptions`].

pub mod agents;
pub mod markers;
pub mod subscriptions;
pub mod tasks;
