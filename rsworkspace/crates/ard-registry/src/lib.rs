#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! ARD-compatible HTTP registry runtime.

pub mod filters;
pub mod http_error;
pub mod lexical_rank;
pub mod page_token;
pub mod registry;
pub mod registry_config;
pub mod registry_error;
pub mod router;
pub mod search_request;

pub use http_error::RegistryHttpError;
pub use registry::Registry;
pub use registry_config::RegistryConfig;
pub use registry_error::RegistryError;
pub use router::router;
