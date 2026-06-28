//! Validated domain request for `POST /explore`.

use ard_catalog::ExploreRequestWire;

use crate::search_filters::SearchFilters;

/// Validated explore request derived from untrusted wire input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedExploreRequest {
    text: Option<String>,
    filters: SearchFilters,
    facet_fields: Vec<String>,
}

impl ValidatedExploreRequest {
    /// Convert an untrusted wire request into a validated domain request.
    ///
    /// This conversion is infallible: whitespace-only text becomes `None`,
    /// absent filters become empty, and facet fields are collected as-is.
    pub fn from_wire(wire: ExploreRequestWire) -> Self {
        let text = wire
            .query
            .as_ref()
            .and_then(|q| q.text.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        let filters = SearchFilters::from_wire(wire.query.and_then(|q| q.filter));

        let facet_fields = wire.result_type.facets.into_iter().map(|f| f.field).collect();

        Self {
            text,
            filters,
            facet_fields,
        }
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn filters(&self) -> &SearchFilters {
        &self.filters
    }

    pub fn facet_fields(&self) -> &[String] {
        &self.facet_fields
    }
}

#[cfg(test)]
mod tests;
