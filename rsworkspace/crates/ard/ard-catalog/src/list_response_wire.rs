//! ARD registry list response wire shape.

use serde::{Deserialize, Serialize};

use crate::catalog_entry_wire::CatalogEntryWire;

/// ARD `GET /agents` response body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponseWire {
    pub items: Vec<CatalogEntryWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
}
