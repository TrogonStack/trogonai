//! Shared mesh identity wire types (`act_chain`, depth limits).

mod act_chain;

pub use act_chain::{ActChainEntry, MAX_ACT_CHAIN_DEPTH, act_chain_has_loop, parse_act_chain};
