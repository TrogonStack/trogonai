//! Typed registry application errors.

use crate::page_token::PageTokenError;
use crate::search_request::SearchRequestError;

/// Error returned by registry application operations.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error(transparent)]
    SearchRequest(#[from] SearchRequestError),
    #[error(transparent)]
    PageToken(#[from] PageTokenError),
    #[error("page size must be between 1 and {max}")]
    InvalidPageSize { max: u32 },
}
