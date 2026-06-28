//! Validated domain request for `GET /agents`.

use ard_catalog::ListAgentsQueryWire;

use crate::page_token::decode_page_token;
use crate::registry_error::RegistryError;
use crate::search_filters::SearchFilters;

pub const DEFAULT_PAGE_SIZE: u32 = 50;
pub const MAX_PAGE_SIZE: u32 = 100;

/// Validated query parameters for the list-agents operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedListAgentsQuery {
    page_size: u32,
    offset: u64,
    filters: SearchFilters,
}

impl ValidatedListAgentsQuery {
    pub fn try_from_wire(wire: ListAgentsQueryWire) -> Result<Self, RegistryError> {
        let page_size = wire.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
        if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
            return Err(RegistryError::InvalidPageSize { max: MAX_PAGE_SIZE });
        }

        let offset = match wire.page_token.as_deref() {
            Some(token) => decode_page_token(token)?,
            None => 0,
        };

        Ok(Self {
            page_size,
            offset,
            filters: SearchFilters::from_wire(wire.filters),
        })
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn filters(&self) -> &SearchFilters {
        &self.filters
    }
}

#[cfg(test)]
mod tests {
    use ard_catalog::ListAgentsQueryWire;

    use super::{DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, ValidatedListAgentsQuery};
    use crate::registry_error::RegistryError;

    #[test]
    fn defaults_page_size() {
        let query = ValidatedListAgentsQuery::try_from_wire(ListAgentsQueryWire {
            page_size: None,
            page_token: None,
            filters: None,
        })
        .unwrap();
        assert_eq!(query.page_size(), DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn rejects_page_size_zero() {
        let wire = ListAgentsQueryWire {
            page_size: Some(0),
            page_token: None,
            filters: None,
        };
        assert!(matches!(
            ValidatedListAgentsQuery::try_from_wire(wire),
            Err(RegistryError::InvalidPageSize { max: MAX_PAGE_SIZE })
        ));
    }

    #[test]
    fn rejects_page_size_over_max() {
        let wire = ListAgentsQueryWire {
            page_size: Some(MAX_PAGE_SIZE + 1),
            page_token: None,
            filters: None,
        };
        assert!(matches!(
            ValidatedListAgentsQuery::try_from_wire(wire),
            Err(RegistryError::InvalidPageSize { max: MAX_PAGE_SIZE })
        ));
    }
}
