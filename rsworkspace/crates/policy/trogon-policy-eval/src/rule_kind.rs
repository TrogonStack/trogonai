use std::fmt;

/// Which policy history a binding lives in.
///
/// Only the kinds this evaluator reads. The variant order is part of the
/// canonical order of refusals and is therefore stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum RuleKind {
    /// `policy/holds`.
    Hold,
    /// `policy/model_ceilings`.
    ModelCeiling,
    /// `policy/delegation_permissions`.
    DelegationPermission,
    /// `policy/egress_permissions`.
    EgressPermission,
}

impl RuleKind {
    /// A stable, human-readable name for logs and refusal text.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::ModelCeiling => "model ceiling",
            Self::DelegationPermission => "delegation permission",
            Self::EgressPermission => "egress permission",
        }
    }
}

impl fmt::Display for RuleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
