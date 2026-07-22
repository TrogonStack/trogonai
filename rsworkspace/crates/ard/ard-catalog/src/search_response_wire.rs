//! ARD registry search response wire shape.

use serde::{Deserialize, Serialize};

use crate::catalog_entry_wire::CatalogEntryWire;
use crate::search_result_wire::SearchResultWire;

/// ARD `POST /search` response body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponseWire {
    pub results: Vec<SearchResultWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrals: Option<Vec<CatalogEntryWire>>,
}
