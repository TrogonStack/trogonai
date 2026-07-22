//! ARD registry facet count wire shape.

use serde::{Deserialize, Serialize};

/// Single facet value count in an explore response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetCountWire {
    pub value: String,
    pub count: u64,
}
