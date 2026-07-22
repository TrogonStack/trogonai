//! ARD registry search result wire shape.

use serde::{Deserialize, Serialize};

use crate::catalog_entry_wire::CatalogEntryWire;

/// Single ARD search result item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultWire {
    #[serde(flatten)]
    pub entry: CatalogEntryWire,
    pub score: u8,
    pub source: String,
}
