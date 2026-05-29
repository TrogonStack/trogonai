use std::time::SystemTime;

use super::config::SchemaCacheConfig;
use super::entry::{CachedSchema, SchemaSource};
use super::errors::SchemaCacheError;
use super::hash::hash_schema;
use super::key::{SchemaCacheKey, ServerId};
use super::store::SchemaCache;

/// Extract `inputSchema` (and optional `outputSchema`) from a successful `tools/list` reply.
pub async fn sniff_tools_list_reply<C: SchemaCache + ?Sized>(
    cache: &C,
    config: &SchemaCacheConfig,
    server_id: &ServerId,
    payload: &[u8],
) -> Result<usize, SchemaCacheError> {
    if !config.enabled {
        return Ok(0);
    }

    let value: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(_) => return Ok(0),
    };

    let Some(tools) = value
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
    else {
        return Ok(0);
    };

    let fetched_at = SystemTime::now();
    let mut stored = 0usize;
    for tool in tools {
        let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if let Some(input_schema) = tool.get("inputSchema").filter(|s| !s.is_null()) {
            store_tool_schema(
                cache,
                server_id,
                name,
                input_schema.clone(),
                fetched_at,
                SchemaSource::ToolsListSniff,
            )
            .await?;
            stored += 1;
        }
        if let Some(output_schema) = tool.get("outputSchema").filter(|s| !s.is_null()) {
            store_tool_schema(
                cache,
                server_id,
                &format!("{name}:output"),
                output_schema.clone(),
                fetched_at,
                SchemaSource::ToolsListSniff,
            )
            .await?;
        }
    }
    Ok(stored)
}

async fn store_tool_schema<C: SchemaCache + ?Sized>(
    cache: &C,
    server_id: &ServerId,
    tool_name: &str,
    schema: serde_json::Value,
    fetched_at: SystemTime,
    source: SchemaSource,
) -> Result<(), SchemaCacheError> {
    let schema_hash = hash_schema(&schema);
    let key = SchemaCacheKey {
        server_id: server_id.clone(),
        schema_hash,
    };
    cache
        .put(
            key,
            CachedSchema {
                schema,
                fetched_at,
                source,
            },
            tool_name,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_cache::store::InMemorySchemaCache;

    #[tokio::test]
    async fn sniff_stores_input_schemas_from_tools_list() {
        let cache = InMemorySchemaCache::new(SchemaCacheConfig::default());
        let server = ServerId::new("fixture");
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": [
                    {
                        "name": "alpha",
                        "inputSchema": {"type": "object", "properties": {"x": {"type": "string"}}}
                    },
                    {
                        "name": "beta",
                        "inputSchema": {"type": "object", "properties": {"y": {"type": "integer"}}}
                    }
                ]
            }
        });
        let count = sniff_tools_list_reply(
            &cache,
            &SchemaCacheConfig::default(),
            &server,
            &serde_json::to_vec(&payload).unwrap(),
        )
        .await
        .expect("sniff");
        assert_eq!(count, 2);
        assert!(cache.lookup_tool(&server, "alpha").await.expect("lookup").is_some());
        assert!(cache.lookup_tool(&server, "beta").await.expect("lookup").is_some());
    }

    #[tokio::test]
    async fn sniff_dedupes_identical_schemas() {
        let cache = InMemorySchemaCache::new(SchemaCacheConfig::default());
        let server = ServerId::new("fixture");
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": [
                    {"name": "one", "inputSchema": {"type": "object"}},
                    {"name": "two", "inputSchema": {"type": "object"}}
                ]
            }
        });
        sniff_tools_list_reply(
            &cache,
            &SchemaCacheConfig::default(),
            &server,
            &serde_json::to_vec(&payload).unwrap(),
        )
        .await
        .expect("sniff");
        assert_eq!(cache.entry_count().await, 1);
    }
}
