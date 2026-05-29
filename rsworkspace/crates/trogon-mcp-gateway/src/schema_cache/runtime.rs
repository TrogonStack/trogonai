use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use tokio::sync::Mutex;

use super::config::SchemaCacheConfig;
use super::entry::CachedSchema;
use super::errors::SchemaCacheError;
use super::hash::hash_schema;
use super::key::{SchemaCacheKey, ServerId};
use super::singleflight::SchemaSingleflight;
use super::sniff::sniff_tools_list_reply;
use super::store::{InMemorySchemaCache, SchemaCache, SharedSchemaCache};

static SHARED: OnceLock<Arc<SchemaCacheRuntime>> = OnceLock::new();

#[derive(Default)]
struct ClientServerBindings {
    by_client: RwLock<HashMap<String, ServerId>>,
}

#[derive(Default)]
struct ServerGenerations {
    versions: RwLock<HashMap<String, u64>>,
}

pub struct SchemaCacheRuntime {
    pub cache: SharedSchemaCache,
    pub config: SchemaCacheConfig,
    pub singleflight: SchemaSingleflight,
    tool_refetch: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    client_servers: ClientServerBindings,
    generations: ServerGenerations,
}

impl SchemaCacheRuntime {
    pub fn new(config: SchemaCacheConfig) -> Arc<Self> {
        Arc::new(Self {
            cache: Arc::new(InMemorySchemaCache::new(config.clone())),
            config,
            singleflight: SchemaSingleflight::default(),
            tool_refetch: Mutex::new(HashMap::new()),
            client_servers: ClientServerBindings::default(),
            generations: ServerGenerations::default(),
        })
    }

    pub fn install(runtime: Arc<Self>) -> Result<(), Arc<SchemaCacheRuntime>> {
        SHARED.set(runtime)
    }

    pub fn shared() -> Option<Arc<Self>> {
        SHARED.get().cloned()
    }

    pub fn record_client_server(&self, client_id: &str, server_id: &ServerId) {
        if client_id.is_empty() {
            return;
        }
        self.client_servers
            .by_client
            .write()
            .expect("client server bindings lock")
            .insert(client_id.to_string(), server_id.clone());
    }

    pub fn server_for_client(&self, client_id: &str) -> Option<ServerId> {
        self.client_servers
            .by_client
            .read()
            .expect("client server bindings lock")
            .get(client_id)
            .cloned()
    }

    pub async fn invalidate_server(&self, server_id: &ServerId) -> Result<(), super::errors::SchemaCacheError> {
        self.cache.invalidate(server_id).await
    }

    pub async fn invalidate_on_reconnect(&self, server_id: &ServerId) -> Result<(), super::errors::SchemaCacheError> {
        {
            let mut versions = self.generations.versions.write().expect("generation lock");
            let counter = versions.entry(server_id.as_str().to_string()).or_insert(0);
            *counter = counter.saturating_add(1);
        }
        self.invalidate_server(server_id).await
    }

    pub fn reconnect_generation(&self, server_id: &str) -> u64 {
        self.generations
            .versions
            .read()
            .expect("generation lock")
            .get(server_id)
            .copied()
            .unwrap_or(0)
    }
}

pub async fn lookup_tool_schema(
    runtime: &SchemaCacheRuntime,
    server_id: &ServerId,
    tool_name: &str,
) -> Result<Option<super::entry::CachedSchema>, super::errors::SchemaCacheError> {
    runtime.cache.lookup_tool(server_id, tool_name).await
}

pub async fn ensure_tool_schema(
    runtime: &SchemaCacheRuntime,
    client: &async_nats::Client,
    prefix: &str,
    server_id: &ServerId,
    tool_name: &str,
    timeout: Duration,
) -> Result<Option<CachedSchema>, SchemaCacheError> {
    if !runtime.config.enabled {
        return Ok(None);
    }
    if let Some(cached) = runtime.cache.lookup_tool(server_id, tool_name).await? {
        return Ok(Some(cached));
    }

    let refetch_key = format!("{}:{}", server_id.as_str(), tool_name);
    let gate = {
        let mut map = runtime.tool_refetch.lock().await;
        map.entry(refetch_key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    let _leader = gate.lock().await;
    if let Some(cached) = runtime.cache.lookup_tool(server_id, tool_name).await? {
        runtime.tool_refetch.lock().await.remove(&refetch_key);
        return Ok(Some(cached));
    }

    let list_subject = format!("{prefix}.server.{server_id}.tools.list");
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "schema-cache-refetch",
        "method": "tools/list",
        "params": {}
    });
    let response = tokio::time::timeout(
        timeout,
        client.request(list_subject, serde_json::to_vec(&payload).unwrap_or_default().into()),
    )
    .await
    .map_err(|_| SchemaCacheError::Backend("tools/list refetch timed out".into()))?
    .map_err(|err| SchemaCacheError::Backend(err.to_string()))?;

    sniff_tools_list_reply(&runtime.cache, &runtime.config, server_id, &response.payload).await?;
    runtime.tool_refetch.lock().await.remove(&refetch_key);
    runtime.cache.lookup_tool(server_id, tool_name).await
}

pub fn schema_cache_key_for_tool(
    server_id: &ServerId,
    schema: &serde_json::Value,
) -> SchemaCacheKey {
    SchemaCacheKey {
        server_id: server_id.clone(),
        schema_hash: hash_schema(schema),
    }
}
