#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! ARD-compatible discovery catalog domain model and wire validation.

pub mod ard_identifier;
pub mod ard_storage_key;
pub mod catalog_entry;
pub mod catalog_entry_wire;
pub mod catalog_host;
pub mod catalog_manifest;
pub mod catalog_manifest_schema;
pub mod catalog_manifest_wire;
pub mod display_name;
pub mod explore_request_wire;
pub mod explore_response_wire;
pub mod facet_count_wire;
pub mod federation_mode;
pub mod list_agents_query_wire;
pub mod list_response_wire;
pub mod media_type;
pub mod metadata;
pub mod registry_error_wire;
pub mod representative_queries;
pub mod search_filters_wire;
pub mod search_request_wire;
pub mod search_response_wire;
pub mod search_result_wire;
pub mod trust_manifest;
pub mod url_or_data;

pub use ard_identifier::{ArdIdentifier, ArdIdentifierError};
pub use ard_storage_key::ArdStorageKey;
pub use catalog_entry::CatalogEntry;
pub use catalog_entry_wire::{CatalogEntryWire, CatalogEntryWireError};
pub use catalog_host::{CatalogHost, CatalogHostError};
pub use catalog_manifest::{CatalogManifest, CatalogManifestJsonError};
pub use catalog_manifest_schema::{AI_CATALOG_JSON_SCHEMA, CatalogManifestValidateError, validate_ai_catalog_value};
pub use catalog_manifest_wire::{CatalogManifestWire, CatalogManifestWireError, SPEC_VERSION};
pub use display_name::{DisplayName, DisplayNameError};
pub use explore_request_wire::{ExploreFacetRequestWire, ExploreQueryWire, ExploreRequestWire, ExploreResultTypeWire};
pub use explore_response_wire::{ExploreFacetResultWire, ExploreResponseWire, ExploreResultTypeNameWire};
pub use facet_count_wire::FacetCountWire;
pub use federation_mode::FederationMode;
pub use list_agents_query_wire::ListAgentsQueryWire;
pub use list_response_wire::ListResponseWire;
pub use media_type::{MediaType, MediaTypeError};
pub use metadata::{Metadata, MetadataError};
pub use registry_error_wire::{RegistryErrorBodyWire, RegistryErrorResponseWire};
pub use representative_queries::{RepresentativeQueries, RepresentativeQueriesError};
pub use search_filters_wire::SearchFiltersWire;
pub use search_request_wire::{SearchQueryWire, SearchRequestWire};
pub use search_response_wire::SearchResponseWire;
pub use search_result_wire::SearchResultWire;
pub use trust_manifest::{TrustManifest, TrustManifestError};
pub use url_or_data::{UrlOrData, UrlOrDataError};
