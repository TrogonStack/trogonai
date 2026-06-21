#![doc = include_str!("../README.md")]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod error;
pub mod identity;

pub use error::BridgeError;
pub use identity::{BridgeAgentId, BridgeUserJwt, CallerHttpsAuth, MintedCallerId};
