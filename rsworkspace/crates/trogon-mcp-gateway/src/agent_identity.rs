//! Shared agent-identity types (mode, delegation chain entries).

pub use trogon_identity_types::{ActChainEntry, MAX_ACT_CHAIN_DEPTH};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AgentIdentityMode {
    #[default]
    Off,
    Shadow,
    Enforce,
}
