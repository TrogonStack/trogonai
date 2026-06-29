//! Bridge JSON-RPC requests over stdin/stdout to an
//! [`A2aClient`](a2a_nats::client::A2aClient).
//!
//! The binary reads newline-delimited JSON-RPC 2.0 requests from stdin,
//! dispatches each one to the matching A2aClient method, and writes the
//! response (or streamed notifications for `message/stream` /
//! `tasks/resubscribe`) back to stdout.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod dispatch;
pub mod io_loop;
pub mod runtime;
pub mod wire;

pub use runtime::{RuntimeError, run};
