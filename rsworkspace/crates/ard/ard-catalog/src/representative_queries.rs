//! ARD representative query list value object.

use std::sync::Arc;

const MIN_REPRESENTATIVE_QUERIES: usize = 2;
const MAX_REPRESENTATIVE_QUERIES: usize = 5;

/// Error returned when [`RepresentativeQueries`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepresentativeQueriesError {
    #[error("representativeQueries must contain at least 2 items (found {count})")]
    TooFew { count: usize },
    #[error("representativeQueries must contain at most 5 items (found {count})")]
    TooMany { count: usize },
    #[error("representativeQueries entries must not be empty")]
    EmptyEntry,
}

/// Validated representative query list for ARD conformance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepresentativeQueries {
    queries: Vec<Arc<str>>,
}

impl RepresentativeQueries {
    pub fn new(queries: Vec<String>) -> Result<Self, RepresentativeQueriesError> {
        if queries.len() < MIN_REPRESENTATIVE_QUERIES {
            return Err(RepresentativeQueriesError::TooFew { count: queries.len() });
        }
        if queries.len() > MAX_REPRESENTATIVE_QUERIES {
            return Err(RepresentativeQueriesError::TooMany { count: queries.len() });
        }
        let mut validated = Vec::with_capacity(queries.len());
        for query in queries {
            if query.trim().is_empty() {
                return Err(RepresentativeQueriesError::EmptyEntry);
            }
            validated.push(Arc::from(query));
        }
        Ok(Self { queries: validated })
    }

    pub fn as_slice(&self) -> &[Arc<str>] {
        &self.queries
    }
}

#[cfg(test)]
mod tests;
