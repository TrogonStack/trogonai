use crate::callee::Callee;
use crate::egress_host_name::EgressHostName;
use crate::model_id::ModelId;
use crate::port::Port;

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
