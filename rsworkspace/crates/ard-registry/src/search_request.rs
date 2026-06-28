//! Validated registry search request values.

use ard_catalog::{FederationMode, SearchFiltersWire, SearchRequestWire};

/// Error returned when validating an untrusted search request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchRequestError {
    #[error("query must not be empty")]
    EmptyQuery,
    #[error("search limit must be between 1 and {max}")]
    InvalidLimit { max: u32 },
}

/// Validated search request for registry operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSearchRequest {
    query: String,
    limit: u32,
    offset: u64,
    federation: FederationMode,
    filters: Option<SearchFiltersWire>,
}

impl ValidatedSearchRequest {
    pub const DEFAULT_LIMIT: u32 = 10;
    pub const MAX_LIMIT: u32 = 100;

    pub fn try_from_wire(wire: SearchRequestWire, offset: u64) -> Result<Self, SearchRequestError> {
        if wire.query.text.trim().is_empty() {
            return Err(SearchRequestError::EmptyQuery);
        }

        let limit = wire.page_size.unwrap_or(Self::DEFAULT_LIMIT);
        if !(1..=Self::MAX_LIMIT).contains(&limit) {
            return Err(SearchRequestError::InvalidLimit { max: Self::MAX_LIMIT });
        }

        Ok(Self {
            query: wire.query.text,
            limit,
            offset,
            federation: wire.federation,
            filters: wire.query.filter,
        })
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn federation(&self) -> FederationMode {
        self.federation
    }

    pub fn filters(&self) -> Option<&SearchFiltersWire> {
        self.filters.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use ard_catalog::{FederationMode, SearchQueryWire, SearchRequestWire};

    use super::{SearchRequestError, ValidatedSearchRequest};

    #[test]
    fn rejects_empty_query() {
        let wire = SearchRequestWire {
            query: SearchQueryWire {
                text: "   ".to_owned(),
                filter: None,
            },
            page_size: None,
            page_token: None,
            federation: FederationMode::None,
        };
        assert_eq!(
            ValidatedSearchRequest::try_from_wire(wire, 0),
            Err(SearchRequestError::EmptyQuery)
        );
    }

    #[test]
    fn defaults_limit() {
        let wire = SearchRequestWire {
            query: SearchQueryWire {
                text: "assistant".to_owned(),
                filter: None,
            },
            page_size: None,
            page_token: None,
            federation: FederationMode::None,
        };
        let request = ValidatedSearchRequest::try_from_wire(wire, 0).unwrap();
        assert_eq!(request.limit(), ValidatedSearchRequest::DEFAULT_LIMIT);
    }
}
