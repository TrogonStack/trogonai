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
mod tests {
    use ard_catalog::SearchFiltersWire;

    use super::SearchFilters;

    #[test]
    fn from_wire_none_is_empty() {
        let filters = SearchFilters::from_wire(None);
        assert!(filters.is_empty());
        assert!(filters.media_types().is_empty());
        assert!(filters.tags().is_empty());
        assert!(filters.capabilities().is_empty());
    }

    #[test]
    fn from_wire_maps_each_field() {
        let wire = SearchFiltersWire {
            media_type: Some(vec!["application/a2a-agent-card+json".to_owned()]),
            tags: Some(vec!["demo".to_owned()]),
            capabilities: Some(vec!["chat".to_owned()]),
        };
        let filters = SearchFilters::from_wire(Some(wire));
        assert!(!filters.is_empty());
        assert_eq!(filters.media_types(), &["application/a2a-agent-card+json"]);
        assert_eq!(filters.tags(), &["demo"]);
        assert_eq!(filters.capabilities(), &["chat"]);
    }

    #[test]
    fn from_wire_absent_fields_become_empty_vecs() {
        let wire = SearchFiltersWire {
            media_type: None,
            tags: Some(vec!["coding".to_owned()]),
            capabilities: None,
        };
        let filters = SearchFilters::from_wire(Some(wire));
        assert!(filters.media_types().is_empty());
        assert_eq!(filters.tags(), &["coding"]);
        assert!(filters.capabilities().is_empty());
    }
}
