//! Subject typestate marker traits + per-operation subject modules.
//!
//! Each subject type implements one of [`markers::Requestable`],
//! [`markers::Publishable`], [`markers::Subscribable`], or [`markers::JetStreamEvents`]
//! so call-sites can't accidentally `request()` a fire-and-forget subject. Per-operation
//! subject types (`MessageSendSubject`, `TaskEventsSubject`, …) land in their dedicated
//! PRs under [`agents`], [`tasks`], and [`subscriptions`].

#![cfg_attr(
    dylint_lib = "trogon_lints",
    expect(
        acyclic_modules,
        reason = "a stream names the subjects it captures and each subject names the stream it is captured by"
    )
)]

pub mod agents;
pub mod markers;
pub mod stream;
pub mod subscriptions;
pub mod tasks;

pub use stream::{A2aStream, StreamAssignment};

#[cfg(test)]
mod conformance_tests;
