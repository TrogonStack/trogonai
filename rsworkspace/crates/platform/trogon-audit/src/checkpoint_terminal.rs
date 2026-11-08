use crate::constants::TERMINAL_SIGNED;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckpointTerminal {
    Signed,
}

impl CheckpointTerminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Signed => TERMINAL_SIGNED,
        }
    }
}
