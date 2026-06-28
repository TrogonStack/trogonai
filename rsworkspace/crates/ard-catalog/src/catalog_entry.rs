//! Validated ARD catalog entry domain model.

use crate::ard_identifier::ArdIdentifier;
use crate::catalog_entry_wire::{CatalogEntryWire, CatalogEntryWireError};
use crate::display_name::DisplayName;
use crate::media_type::MediaType;
use crate::metadata::Metadata;
use crate::representative_queries::RepresentativeQueries;
use crate::trust_manifest::TrustManifest;
use crate::url_or_data::UrlOrData;

/// Validated ARD catalog entry.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogEntry {
    identifier: ArdIdentifier,
    display_name: DisplayName,
    media_type: MediaType,
    delivery: UrlOrData,
    description: Option<String>,
    representative_queries: Option<RepresentativeQueries>,
    tags: Option<Vec<String>>,
    capabilities: Option<Vec<String>>,
    version: Option<String>,
    updated_at: Option<String>,
    metadata: Option<Metadata>,
    trust_manifest: Option<TrustManifest>,
}

impl CatalogEntry {
    pub fn try_from_wire(wire: CatalogEntryWire) -> Result<Self, CatalogEntryWireError> {
        if wire.identifier.trim().is_empty() {
            return Err(CatalogEntryWireError::MissingIdentifier);
        }
        if wire.display_name.trim().is_empty() {
            return Err(CatalogEntryWireError::MissingDisplayName);
        }
        if wire.media_type.trim().is_empty() {
            return Err(CatalogEntryWireError::MissingMediaType);
        }

        let identifier = ArdIdentifier::new(wire.identifier)?;
        let display_name = DisplayName::new(wire.display_name)?;
        let media_type = MediaType::new(wire.media_type)?;
        let delivery = UrlOrData::from_optional(wire.url, wire.data)?;
        let representative_queries = match wire.representative_queries {
            Some(queries) => Some(RepresentativeQueries::new(queries)?),
            None => None,
        };
        let trust_manifest = match wire.trust_manifest {
            Some(trust_manifest) => Some(TrustManifest::new(trust_manifest)?),
            None => None,
        };
        let metadata = match wire.metadata {
            Some(metadata) => Some(Metadata::new(metadata)?),
            None => None,
        };
        if let Some(updated_at) = wire.updated_at.as_deref()
            && time::OffsetDateTime::parse(updated_at, &time::format_description::well_known::Rfc3339).is_err()
        {
            return Err(CatalogEntryWireError::InvalidUpdatedAt);
        }

        Ok(Self {
            identifier,
            display_name,
            media_type,
            delivery,
            description: wire.description,
            representative_queries,
            tags: wire.tags,
            capabilities: wire.capabilities,
            version: wire.version,
            updated_at: wire.updated_at,
            metadata,
            trust_manifest,
        })
    }

    pub fn identifier(&self) -> &ArdIdentifier {
        &self.identifier
    }

    pub fn display_name(&self) -> &str {
        self.display_name.as_str()
    }

    pub fn media_type(&self) -> &MediaType {
        &self.media_type
    }

    pub fn delivery(&self) -> &UrlOrData {
        &self.delivery
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn representative_queries(&self) -> Option<&RepresentativeQueries> {
        self.representative_queries.as_ref()
    }

    pub fn tags(&self) -> Option<&[String]> {
        self.tags.as_deref()
    }

    pub fn capabilities(&self) -> Option<&[String]> {
        self.capabilities.as_deref()
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn updated_at(&self) -> Option<&str> {
        self.updated_at.as_deref()
    }

    pub fn metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.as_ref().map(Metadata::as_value)
    }

    pub fn trust_manifest(&self) -> Option<&TrustManifest> {
        self.trust_manifest.as_ref()
    }

    pub fn into_wire(self) -> CatalogEntryWire {
        CatalogEntryWire::from(self)
    }
}

impl TryFrom<CatalogEntryWire> for CatalogEntry {
    type Error = CatalogEntryWireError;

    fn try_from(wire: CatalogEntryWire) -> Result<Self, Self::Error> {
        Self::try_from_wire(wire)
    }
}

impl From<CatalogEntry> for CatalogEntryWire {
    fn from(entry: CatalogEntry) -> Self {
        let (url, data) = match entry.delivery {
            UrlOrData::Url(url) => (Some(url.to_string()), None),
            UrlOrData::Data(data) => (None, Some(data)),
        };
        let representative_queries = entry
            .representative_queries
            .map(|queries| queries.as_slice().iter().map(|query| query.to_string()).collect());
        Self {
            identifier: entry.identifier.to_string(),
            display_name: entry.display_name.to_string(),
            media_type: entry.media_type.to_string(),
            url,
            data,
            description: entry.description,
            representative_queries,
            tags: entry.tags,
            capabilities: entry.capabilities,
            version: entry.version,
            updated_at: entry.updated_at,
            metadata: entry.metadata.map(Metadata::into_value),
            trust_manifest: entry.trust_manifest.map(TrustManifest::into_value),
        }
    }
}

#[cfg(test)]
mod tests;
