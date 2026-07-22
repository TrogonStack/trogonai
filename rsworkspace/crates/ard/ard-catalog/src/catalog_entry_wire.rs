//! Untrusted ARD catalog entry wire shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ard_identifier::ArdIdentifierError;
use crate::display_name::DisplayNameError;
use crate::media_type::MediaTypeError;
use crate::metadata::MetadataError;
use crate::representative_queries::RepresentativeQueriesError;
use crate::trust_manifest::TrustManifestError;
use crate::url_or_data::UrlOrDataError;

/// Error returned when converting [`CatalogEntryWire`] into domain values.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CatalogEntryWireError {
    #[error("identifier is required")]
    MissingIdentifier,
    #[error("displayName is required")]
    MissingDisplayName,
    #[error("type is required")]
    MissingMediaType,
    #[error("updatedAt must be an RFC 3339 date-time")]
    InvalidUpdatedAt,
    #[error(transparent)]
    Identifier(#[from] ArdIdentifierError),
    #[error(transparent)]
    DisplayName(#[from] DisplayNameError),
    #[error(transparent)]
    MediaType(#[from] MediaTypeError),
    #[error(transparent)]
    Delivery(#[from] UrlOrDataError),
    #[error(transparent)]
    RepresentativeQueries(#[from] RepresentativeQueriesError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error(transparent)]
    TrustManifest(#[from] TrustManifestError),
}

/// Untrusted ARD catalog entry JSON shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntryWire {
    pub identifier: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representative_queries: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_manifest: Option<Value>,
}

impl CatalogEntryWire {
    pub fn into_domain(self) -> Result<crate::catalog_entry::CatalogEntry, CatalogEntryWireError> {
        crate::catalog_entry::CatalogEntry::try_from_wire(self)
    }
}

#[cfg(test)]
mod tests;
