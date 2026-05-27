//! Shared agent-identity types (mode, delegation chain entries).

use serde::{Deserialize, Serialize};

pub const MAX_ACT_CHAIN_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AgentIdentityMode {
    #[default]
    Off,
    Shadow,
    Enforce,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ActChainEntry {
    pub sub: String,
    pub agent_id: Option<String>,
    pub wkl: Option<String>,
    pub iat: i64,
}
