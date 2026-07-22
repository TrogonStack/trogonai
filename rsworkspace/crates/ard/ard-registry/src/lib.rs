#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! ARD-compatible HTTP registry runtime.

pub mod explore_request;
pub mod extract;
pub mod facet_field;
pub mod filters;
pub mod http_error;
pub mod lexical_rank;
pub mod list_agents_request;
pub mod page_token;
pub mod registry;
pub mod registry_config;
pub mod registry_error;
pub mod router;
pub mod search_filters;
pub mod search_request;
pub mod source_url;

pub use explore_request::ValidatedExploreRequest;
pub use http_error::RegistryHttpError;
pub use list_agents_request::ValidatedListAgentsQuery;
pub use registry::Registry;
pub use registry_config::RegistryConfig;
pub use registry_error::RegistryError;
pub use router::router;
pub use search_filters::SearchFilters;
pub use search_request::ValidatedSearchRequest;
pub use source_url::SourceUrl;
