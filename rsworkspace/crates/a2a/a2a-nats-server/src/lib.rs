//! Default A2A-over-NATS agent runtime entry point.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod noop_handler;
pub mod runtime;

pub use noop_handler::NoopHandler;
pub use runtime::RuntimeError;
