//! Terminal sessions this server holds for its client.
//!
//! A terminal's output is a live stream on NATS, while an MCP client can only ask and wait for an answer. Each
//! session subscribes to its terminal's output before the terminal opens and keeps what arrives in a bounded
//! buffer that `sandbox_terminal_read` drains.

mod buffer;
mod fresh_terminal_id;
mod read_outcome;
pub(crate) mod session;
mod sessions;

pub(crate) use fresh_terminal_id::fresh_terminal_id;
pub(crate) use read_outcome::ReadOutcome;
pub(crate) use sessions::Sessions;
