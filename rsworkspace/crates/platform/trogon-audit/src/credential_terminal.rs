use crate::constants::{TERMINAL_INJECT, TERMINAL_RELEASE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialTerminal {
    Release,
    Inject,
}

impl CredentialTerminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => TERMINAL_RELEASE,
            Self::Inject => TERMINAL_INJECT,
        }
    }
}
