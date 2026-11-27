use serde::{Deserialize, Serialize};

/// A host-owned classification, never a classification supplied by a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationLocation {
    Local,
    Subscription,
}
