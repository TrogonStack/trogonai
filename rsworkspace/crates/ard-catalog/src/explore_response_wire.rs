//! ARD registry explore response wire shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::facet_count_wire::FacetCountWire;

/// ARD explore result type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExploreResultTypeNameWire {
    Facets,
}

/// ARD facet result bucket set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreFacetResultWire {
    pub buckets: Vec<FacetCountWire>,
    #[serde(default)]
    pub other_count: u64,
}

/// ARD `POST /explore` response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreResponseWire {
    pub result_type: ExploreResultTypeNameWire,
    pub facets: BTreeMap<String, ExploreFacetResultWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
}
