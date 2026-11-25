use crate::rule_name::RuleName;

/// What one request rule says about a request.
///
/// There is deliberately no arm that permits. A rule that answers `Pass` has
/// said only that it does not refuse; it can never stand in for a missing permit
/// (ADR#0071 Decision 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Narrowing {
    /// The rule does not refuse the request.
    Pass,
    /// The rule refuses the request, and this is the one rule that did.
    Deny(RuleName),
}

impl Narrowing {
    /// The refusing rule, if any.
    pub fn denial(&self) -> Option<&RuleName> {
        match self {
            Self::Pass => None,
            Self::Deny(rule) => Some(rule),
        }
    }
}
