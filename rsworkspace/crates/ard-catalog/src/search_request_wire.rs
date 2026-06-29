//! Untrusted ARD registry search request wire shape.

use serde::{Deserialize, Serialize};

use crate::federation_mode::FederationMode;
use crate::search_filters_wire::SearchFiltersWire;

/// Untrusted ARD search query object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQueryWire {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<SearchFiltersWire>,
}

/// Untrusted ARD `POST /search` request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequestWire {
    pub query: SearchQueryWire,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
    #[serde(default)]
    pub federation: FederationMode,
}
