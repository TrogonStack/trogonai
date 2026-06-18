//! Client-side scaffolding for A2A callers.
//!
//! Per-operation methods (`message_send`, `tasks_get`, `message_stream`, …) land in
//! their dedicated PRs so each operation's wire contract is reviewed on its own.

pub mod error;
pub mod event_stream;
pub mod gateway_headers;
pub mod handle;
pub mod resubscribe;
pub mod streaming;
pub mod unary;
pub mod wire;

pub use error::ClientError;
pub use handle::A2aClient;
