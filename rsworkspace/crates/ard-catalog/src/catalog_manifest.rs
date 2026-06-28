//! Validated ARD catalog manifest domain model.

use crate::catalog_entry::CatalogEntry;
use crate::catalog_entry_wire::CatalogEntryWire;
use crate::catalog_host::CatalogHost;
use crate::catalog_manifest_schema::{CatalogManifestValidateError, validate_ai_catalog_value};
use crate::catalog_manifest_wire::{CatalogManifestWire, CatalogManifestWireError, SPEC_VERSION};

/// Error returned when parsing a JSON manifest through schema and domain validation.
#[derive(Debug, thiserror::Error)]
pub enum CatalogManifestJsonError {
    #[error(transparent)]
    Schema(#[from] CatalogManifestValidateError),
    #[error(transparent)]
    Decode(#[from] serde_json::Error),
    #[error(transparent)]
    Domain(#[from] CatalogManifestWireError),
}

/// Validated ARD catalog manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogManifest {
    host: Option<CatalogHost>,
    entries: Vec<CatalogEntry>,
}

impl CatalogManifest {
    pub fn try_from_wire(wire: CatalogManifestWire) -> Result<Self, CatalogManifestWireError> {
        if wire.spec_version != SPEC_VERSION {
            return Err(CatalogManifestWireError::InvalidSpecVersion);
        }
        let mut entries = Vec::with_capacity(wire.entries.len());
        for (index, entry) in wire.entries.into_iter().enumerate() {
            entries.push(
                CatalogEntry::try_from_wire(entry)
                    .map_err(|source| CatalogManifestWireError::Entry { index, source })?,
            );
        }
        let host = match wire.host {
            Some(host) => Some(CatalogHost::new(host)?),
            None => None,
        };
        Ok(Self { host, entries })
    }

    pub fn from_json_value(value: serde_json::Value) -> Result<Self, CatalogManifestJsonError> {
        validate_ai_catalog_value(&value)?;
        let wire: CatalogManifestWire = serde_json::from_value(value)?;
        Ok(Self::try_from_wire(wire)?)
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn host(&self) -> Option<&serde_json::Value> {
        self.host.as_ref().map(CatalogHost::as_value)
    }

    pub fn into_entries(self) -> Vec<CatalogEntry> {
        self.entries
    }

    pub fn spec_version(&self) -> &'static str {
        SPEC_VERSION
    }

    pub fn into_wire(self) -> CatalogManifestWire {
        CatalogManifestWire::from(self)
    }

    pub fn into_json_value(self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self.into_wire())
    }
}

impl TryFrom<CatalogManifestWire> for CatalogManifest {
    type Error = CatalogManifestWireError;

    fn try_from(wire: CatalogManifestWire) -> Result<Self, Self::Error> {
        Self::try_from_wire(wire)
    }
}

impl From<CatalogManifest> for CatalogManifestWire {
    fn from(manifest: CatalogManifest) -> Self {
        let CatalogManifest { host, entries } = manifest;
        Self {
            spec_version: SPEC_VERSION.to_owned(),
            host: host.map(CatalogHost::into_value),
            entries: entries.into_iter().map(CatalogEntryWire::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests;
