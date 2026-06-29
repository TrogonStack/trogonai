//! Validated domain search filter value object.

use ard_catalog::{MediaType, SearchFiltersWire};

use crate::registry_error::RegistryError;

/// Validated search filters derived from untrusted wire input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchFilters {
    media_types: Vec<MediaType>,
    tags: Vec<String>,
    capabilities: Vec<String>,
}

impl SearchFilters {
    /// Convert an optional wire filter into validated domain filters.
    ///
    /// Returns an error if any media type string is invalid.
    /// Absent or `None` wire values become empty vectors.
    pub fn try_from_wire(wire: Option<SearchFiltersWire>) -> Result<Self, RegistryError> {
        match wire {
            None => Ok(Self::default()),
            Some(w) => {
                let media_types = w
                    .media_type
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| MediaType::new(s).map_err(RegistryError::from))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self {
                    media_types,
                    tags: w.tags.unwrap_or_default(),
                    capabilities: w.capabilities.unwrap_or_default(),
                })
            }
        }
    }

    pub fn media_types(&self) -> &[MediaType] {
        &self.media_types
    }

    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Returns `true` when all three filter dimensions are empty (no filtering).
    pub fn is_empty(&self) -> bool {
        self.media_types.is_empty() && self.tags.is_empty() && self.capabilities.is_empty()
    }
}

#[cfg(test)]
mod tests;
