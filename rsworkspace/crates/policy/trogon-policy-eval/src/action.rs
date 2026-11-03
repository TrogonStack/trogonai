use crate::callee::Callee;
use crate::egress_host_name::EgressHostName;
use crate::model_id::ModelId;
use crate::port::Port;
use crate::rule_kind::RuleKind;

/// What is being asked for.
///
/// Open to growth: a tool or connection target ships only when the resource it
/// names has a contract (ADR#0071 Decision 7).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// Open a connection to an exact host and port.
    EgressConnect {
        /// The destination host.
        host: EgressHostName,
        /// The destination port.
        port: Port,
    },
    /// Select a catalog model.
    ModelUse {
        /// The selected model.
        model: ModelId,
    },
    /// Dispatch a Session to an Agent that the selector already resolved.
    Delegate {
        /// The resolved Agent and the nodes it sits under.
        callee: Callee,
    },
}

impl Action {
    /// The permit history that must cover this action, if any.
    ///
    /// Model selection has none here: ADR#0032 authorizes models through a
    /// connection's allowed models and ADR#0071 Decision 5 makes the ceiling a
    /// narrowing of that grant, not a second copy of it. The permit for a model
    /// selection is the connection's own list, which is checked where the
    /// connection is resolved.
    pub(crate) fn required_permit(&self) -> Option<RuleKind> {
        match self {
            Self::EgressConnect { .. } => Some(RuleKind::EgressPermission),
            Self::ModelUse { .. } => None,
            Self::Delegate { .. } => Some(RuleKind::DelegationPermission),
        }
    }
}
