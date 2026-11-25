use crate::constants::{TERMINAL_CREDENTIAL, TERMINAL_POLICY};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeTerminal {
    Policy,
    Credential,
}

impl ChangeTerminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Policy => TERMINAL_POLICY,
            Self::Credential => TERMINAL_CREDENTIAL,
        }
    }
}
