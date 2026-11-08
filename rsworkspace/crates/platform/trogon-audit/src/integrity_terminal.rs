use crate::constants::{TERMINAL_CHAIN_RESET, TERMINAL_START};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntegrityTerminal {
    Start,
    ChainReset,
}

impl IntegrityTerminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => TERMINAL_START,
            Self::ChainReset => TERMINAL_CHAIN_RESET,
        }
    }
}
