//! Validated registry search request values.

use ard_catalog::{FederationMode, SearchRequestWire};

use crate::page_token::decode_page_token;
use crate::registry_error::RegistryError;
use crate::search_filters::SearchFilters;

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
    filters: SearchFilters,
}

impl ValidatedSearchRequest {
    pub const DEFAULT_LIMIT: u32 = 10;
    pub const MAX_LIMIT: u32 = 100;

    pub fn try_from_wire(wire: SearchRequestWire) -> Result<Self, RegistryError> {
        if wire.query.text.trim().is_empty() {
            return Err(SearchRequestError::EmptyQuery.into());
        }

        let limit = wire.page_size.unwrap_or(Self::DEFAULT_LIMIT);
        if !(1..=Self::MAX_LIMIT).contains(&limit) {
            return Err(SearchRequestError::InvalidLimit { max: Self::MAX_LIMIT }.into());
        }

        let offset = match wire.page_token.as_deref() {
            Some(token) => decode_page_token(token)?,
            None => 0,
        };

        Ok(Self {
            query: wire.query.text,
            limit,
            offset,
            federation: wire.federation,
            filters: SearchFilters::from_wire(wire.query.filter),
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

    pub fn filters(&self) -> &SearchFilters {
        &self.filters
    }
}

#[cfg(test)]
mod tests;
