//! API key storage and authorization checks shared by the sandbox services.
//!
//! - API keys: [`BearerToken`] is the secret a caller presents, [`ApiKeyHash`] is what the store
//!   indexes it under, and an [`ApiKeyStore`] resolves the hash to an [`ApiKeyRecord`].
//! - Authorization: an [`Authorizer`] decides whether a service account may lease sandboxes for a
//!   tenant; [`SpiceDbAuthorizer`] asks an external permission service.

mod api_key_bucket;
mod api_key_bucket_error;
mod api_key_hash;
mod api_key_hash_error;
mod api_key_record;
mod api_key_record_error;
mod api_key_record_wire;
mod api_key_store;
mod api_key_store_error;
mod authorizer;
mod authz_error;
mod bearer_token;
mod bearer_token_error;
mod in_memory_api_key_store;
mod nats_kv_api_key_store;
mod noop_authorizer;
mod sandbox_authz_request;
mod spicedb_authorizer;
mod spicedb_client;
mod spicedb_connect_error;
mod spicedb_endpoint;
mod spicedb_endpoint_error;
mod spicedb_token;
mod spicedb_token_error;
mod tenant_resource_type;
mod tenant_resource_type_error;

pub use api_key_bucket::{ApiKeyBucket, DEFAULT_BUCKET};
pub use api_key_bucket_error::ApiKeyBucketError;
pub use api_key_hash::ApiKeyHash;
pub use api_key_hash_error::ApiKeyHashError;
pub use api_key_record::ApiKeyRecord;
pub use api_key_record_error::ApiKeyRecordError;
pub use api_key_store::ApiKeyStore;
pub use api_key_store_error::ApiKeyStoreError;
pub use authorizer::Authorizer;
pub use authz_error::AuthzError;
pub use bearer_token::BearerToken;
pub use bearer_token_error::BearerTokenError;
pub use in_memory_api_key_store::InMemoryApiKeyStore;
pub use nats_kv_api_key_store::NatsKvApiKeyStore;
pub use noop_authorizer::NoopAuthorizer;
pub use sandbox_authz_request::SandboxAuthzRequest;
pub use spicedb_authorizer::SpiceDbAuthorizer;
pub use spicedb_client::SpiceDbClient;
pub use spicedb_connect_error::SpiceDbConnectError;
pub use spicedb_endpoint::SpiceDbEndpoint;
pub use spicedb_endpoint_error::SpiceDbEndpointError;
pub use spicedb_token::SpiceDbToken;
pub use spicedb_token_error::SpiceDbTokenError;
pub use tenant_resource_type::TenantResourceType;
pub use tenant_resource_type_error::TenantResourceTypeError;
