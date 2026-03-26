#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod connection;

pub use connection::{AgentSideNatsConnection, ConnectionError};
