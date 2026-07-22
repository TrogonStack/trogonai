//! Untrusted ARD registry explore request wire shape.

use serde::{Deserialize, Serialize};

use crate::search_filters_wire::SearchFiltersWire;

/// Untrusted ARD explore query object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreQueryWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<SearchFiltersWire>,
}

/// Requested ARD explore result type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreResultTypeWire {
    pub facets: Vec<ExploreFacetRequestWire>,
}

/// Requested ARD explore facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreFacetRequestWire {
    pub field: String,
}

/// Untrusted ARD `POST /explore` request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreRequestWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<ExploreQueryWire>,
    pub result_type: ExploreResultTypeWire,
}
