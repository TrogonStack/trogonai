use crate::decision_terminal::DecisionTerminal;
use crate::enforcement::Enforcement;

/// What a layer or point did to an act.
///
/// The three states are the only coherent ones: an enforced refusal, a DryRun
/// refusal that was forwarded, and an allow under either mode. A denial that is
/// both refused and not enforced cannot be spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecisionOutcome {
    Allow(Enforcement),
    Deny,
    WouldDeny,
}

impl DecisionOutcome {
    pub fn enforcement(self) -> Enforcement {
        match self {
            Self::Allow(enforcement) => enforcement,
            Self::Deny => Enforcement::Enforce,
            Self::WouldDeny => Enforcement::DryRun,
        }
    }

    pub fn terminal(self) -> DecisionTerminal {
        match self {
            Self::Allow(_) => DecisionTerminal::Allow,
            Self::Deny => DecisionTerminal::Deny,
            Self::WouldDeny => DecisionTerminal::WouldDeny,
        }
    }

    /// True when the act was refused, so the record describes something that did
    /// not happen and "a deny stands" applies.
    pub fn is_refusal(self) -> bool {
        matches!(self, Self::Deny)
    }
}

#[cfg(test)]
mod tests;
