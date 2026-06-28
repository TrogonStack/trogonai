//! Untrusted ARD catalog manifest wire shape.

use serde::{Deserialize, Serialize};

use crate::catalog_entry_wire::{CatalogEntryWire, CatalogEntryWireError};
use crate::catalog_host::CatalogHostError;
use crate::catalog_host_wire::CatalogHostWire;

/// Pinned ARD manifest spec version for this crate snapshot.
pub const SPEC_VERSION: &str = "1.0";

/// Error returned when converting [`CatalogManifestWire`] into domain values.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CatalogManifestWireError {
    #[error("specVersion must be {SPEC_VERSION}")]
    InvalidSpecVersion,
    #[error(transparent)]
    Host(#[from] CatalogHostError),
    #[error("entry {index}: {source}")]
    Entry {
        index: usize,
        #[source]
        source: CatalogEntryWireError,
    },
}

/// Untrusted ARD `ai-catalog.json` wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogManifestWire {
    pub spec_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<CatalogHostWire>,
    pub entries: Vec<CatalogEntryWire>,
}

impl CatalogManifestWire {
    pub fn into_domain(self) -> Result<crate::catalog_manifest::CatalogManifest, CatalogManifestWireError> {
        crate::catalog_manifest::CatalogManifest::try_from_wire(self)
    }
}

#[cfg(test)]
mod tests;
