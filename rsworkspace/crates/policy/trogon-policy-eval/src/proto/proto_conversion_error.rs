use crate::egress_host_name::EgressHostNameError;
use crate::identifier::IdentifierError;
use crate::port::PortError;

/// Why a generated policy message cannot become a value object of this crate.
///
/// A projection that meets one of these has read state the contract does not
/// allow. It must not guess at what the state meant, so it leaves the history
/// out of the snapshot and lets the staleness bound refuse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoConversionError {
    /// A message or oneof arm the contract requires is not set.
    #[error("{message} is missing its required `{field}`")]
    MissingField {
        /// The message that lacks the field.
        message: &'static str,
        /// The field, or the oneof, that is not set.
        field: &'static str,
    },
    /// The enum value is unspecified, or a number this build does not know.
    #[error("{enumeration} value {value} is unspecified or unknown")]
    UnknownEnumValue {
        /// The enum the value was read as.
        enumeration: &'static str,
        /// The number on the wire.
        value: i32,
    },
    /// The kind is a real one in the schema, and this evaluator does not read
    /// that history.
    #[error("policy rule kind {value} is not read by this evaluator")]
    UnsupportedRuleKind {
        /// The number on the wire.
        value: i32,
    },
    /// The schema calls an empty policy invalid and withdraws a permission or
    /// a ceiling by clearing it. An empty list is not read as an empty set,
    /// because for a ceiling that would silently mean "admits nothing".
    #[error("{policy} must name at least one entry")]
    EmptyPolicy {
        /// The policy message that was empty.
        policy: &'static str,
    },
    /// The Agent identifier is not a valid identity.
    #[error("invalid agent id: {0}")]
    InvalidAgentId(#[source] IdentifierError),
    /// The node identifier is not a valid identity.
    #[error("invalid node id: {0}")]
    InvalidNodeId(#[source] IdentifierError),
    /// The model identifier is not a valid identity.
    #[error("invalid model id: {0}")]
    InvalidModelId(#[source] IdentifierError),
    /// The hold name is not a valid name.
    #[error("invalid hold name: {0}")]
    InvalidHoldName(#[source] IdentifierError),
    /// The host is not an exact plain host name.
    #[error("invalid egress host: {0}")]
    InvalidHost(#[from] EgressHostNameError),
    /// The port is zero or above 65535.
    #[error("invalid egress port: {0}")]
    InvalidPort(#[from] PortError),
}
