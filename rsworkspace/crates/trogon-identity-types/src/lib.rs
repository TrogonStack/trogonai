//! Shared mesh identity wire types (`act_chain`, depth limits, AAuth claims).

mod act_chain;
pub mod aauth;

pub use act_chain::{ActChainEntry, MAX_ACT_CHAIN_DEPTH, act_chain_has_loop, parse_act_chain};
