//! Untrusted ARD search filter wire shape.

use serde::{Deserialize, Serialize};

/// Optional filters applied to registry search and explore requests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFiltersWire {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub media_type: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}
