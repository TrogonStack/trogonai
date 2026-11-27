use serde::{Deserialize, Serialize};

/// What the egress proxy did with a connection it reported as a violation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressDecision {
    Deny,
}
