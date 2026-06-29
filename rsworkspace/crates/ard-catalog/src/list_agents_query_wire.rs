//! Untrusted ARD `GET /agents` query wire shape.

use serde::{Deserialize, Serialize};

use crate::search_filters_wire::SearchFiltersWire;

/// Untrusted ARD `GET /agents` query parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsQueryWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<SearchFiltersWire>,
}
