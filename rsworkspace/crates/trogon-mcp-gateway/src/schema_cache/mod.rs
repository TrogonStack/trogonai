//! Cached JSON schemas from backend MCP `tools/list` responses for validation and redaction.

mod config;
mod entry;
mod errors;
mod hash;
mod invalidate;
mod key;
mod runtime;
mod singleflight;
mod sniff;
mod store;

pub use config::SchemaCacheConfig;
pub use entry::{CachedSchema, SchemaSource};
pub use errors::SchemaCacheError;
pub use hash::{canonical_json_bytes, hash_schema};
pub use invalidate::{
    handle_control_invalidate, handle_list_changed_notification, notification_method,
    parse_client_id_from_notification_subject, should_invalidate,
};
pub use key::{SchemaCacheKey, SchemaHash, SchemaHashError, ServerId};
pub use runtime::{SchemaCacheRuntime, ensure_tool_schema, lookup_tool_schema, schema_cache_key_for_tool};
pub use singleflight::SchemaSingleflight;
pub use sniff::sniff_tools_list_reply;
pub use store::{InMemorySchemaCache, SchemaCache, SharedSchemaCache};
