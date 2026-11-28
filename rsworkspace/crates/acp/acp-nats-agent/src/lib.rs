#![doc = include_str!("../README.md")]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod client_gate;
mod connection;
pub mod constants;

pub use client_gate::{AdmitAll, ClientGate};
pub use connection::{AgentSideNatsConnection, ConnectionError};
