//! Validated domain search filter value object.

use ard_catalog::SearchFiltersWire;

/// Validated search filters derived from untrusted wire input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchFilters {
    media_types: Vec<String>,
    tags: Vec<String>,
    capabilities: Vec<String>,
}

impl SearchFilters {
    /// Convert an optional wire filter into validated domain filters.
    ///
    /// Absent or `None` wire values become empty vectors.
    pub fn from_wire(wire: Option<SearchFiltersWire>) -> Self {
        match wire {
            None => Self::default(),
            Some(w) => Self {
                media_types: w.media_type.unwrap_or_default(),
                tags: w.tags.unwrap_or_default(),
                capabilities: w.capabilities.unwrap_or_default(),
            },
        }
    }

    pub fn media_types(&self) -> &[String] {
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
