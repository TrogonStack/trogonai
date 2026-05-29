use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainResolutionError {
    Revoked { index: usize, agent_id: String },
    Unknown { index: usize, agent_id: String },
    RegistryUnavailable,
    MalformedEntry { index: usize },
}

impl fmt::Display for ChainResolutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Revoked { index, agent_id } => {
                write!(f, "act_chain entry {index} agent_id {agent_id} is revoked")
            }
            Self::Unknown { index, agent_id } => {
                write!(f, "act_chain entry {index} agent_id {agent_id} is unknown")
            }
            Self::RegistryUnavailable => f.write_str("agent registry unavailable"),
            Self::MalformedEntry { index } => write!(f, "act_chain entry {index} is malformed"),
        }
    }
}

impl std::error::Error for ChainResolutionError {}
