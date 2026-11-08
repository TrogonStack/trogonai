use crate::constants::{TERMINAL_ALLOW, TERMINAL_DENY, TERMINAL_WOULD_DENY};

/// `would_deny` is a DryRun refusal that was forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionTerminal {
    Allow,
    Deny,
    WouldDeny,
}

impl DecisionTerminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => TERMINAL_ALLOW,
            Self::Deny => TERMINAL_DENY,
            Self::WouldDeny => TERMINAL_WOULD_DENY,
        }
    }
}
