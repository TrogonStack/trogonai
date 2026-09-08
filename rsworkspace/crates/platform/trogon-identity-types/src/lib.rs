//! Shared mesh identity wire types (`act_chain`, depth limits, AAuth claims).
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod aauth;
mod act_chain;
pub mod constants;

pub use act_chain::{ActChainEntry, act_chain_has_loop, parse_act_chain};
pub use constants::MAX_ACT_CHAIN_DEPTH;
