//! Validated domain request for `POST /explore`.

use ard_catalog::ExploreRequestWire;

use crate::facet_field::FacetField;
use crate::registry_error::RegistryError;
use crate::search_filters::SearchFilters;

/// Validated explore request derived from untrusted wire input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedExploreRequest {
    text: Option<String>,
    filters: SearchFilters,
    facet_fields: Vec<FacetField>,
}

impl ValidatedExploreRequest {
    /// Convert an untrusted wire request into a validated domain request.
    ///
    /// Returns an error if any facet field or media type filter is invalid.
    pub fn try_from_wire(wire: ExploreRequestWire) -> Result<Self, RegistryError> {
        let text = wire
            .query
            .as_ref()
            .and_then(|q| q.text.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        let filters = SearchFilters::try_from_wire(wire.query.and_then(|q| q.filter))?;

        let facet_fields = wire
            .result_type
            .facets
            .into_iter()
            .map(|f| FacetField::parse(&f.field).map_err(RegistryError::from))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            text,
            filters,
            facet_fields,
        })
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn filters(&self) -> &SearchFilters {
        &self.filters
    }

    pub fn facet_fields(&self) -> &[FacetField] {
        &self.facet_fields
    }
}

#[cfg(test)]
mod tests;
