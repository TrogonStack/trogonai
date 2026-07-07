//! Agent delegation (`act` claim) per draft sections "Agent Delegation" and
//! "Delegation Chain".

use serde::{Deserialize, Serialize};

/// Delegation chain entry recorded in an auth token's `act` claim. Modeled on
/// RFC 8693 Section 4.1, but AAuth uses `agent` (not RFC 8693's `sub`) as the
/// identifier field, making explicit that the value is an AAuth agent identifier.
/// See "Delegation Chain" for the recursive nesting rule and examples.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Act {
    /// The `aauth:` URI of the immediate upstream agent: the intermediary resource
    /// in call chaining, or the parent agent in sub-agent authorization.
    pub agent: String,
    /// Nested upstream delegation, present when the upstream agent was itself
    /// delegated to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<Box<Act>>,
}

#[cfg(test)]
mod tests;
