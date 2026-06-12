//! Client for the NATS-backed model catalog (keystone S1).
//!
//! Exposes the two shared primitives consumed by CLI and IDE builders:
//! - [`compactable_models`] — pure predicate over a cached snapshot
//! - [`codec`] — provider-qualified `qualify` / `parse` / `resolve`

mod catalog_entry;
mod codec;
mod config;
mod error;
mod predicate;
mod provision;
mod snapshot;
mod store;

pub use catalog_entry::CatalogEntry;
pub use codec::{
    CodecError, QualifiedModel, backfill_compactor_provider, parse, qualify, resolve,
};
pub use config::CatalogClientConfig;
pub use error::CatalogClientError;
pub use predicate::{CompactableFilter, compactable_models};
pub use provision::{BUCKET_NAME, CATALOG_KEY, PROVIDERS_KEY, provision};
pub use snapshot::CatalogSnapshot;
pub use store::CatalogStore;
#[cfg(any(test, feature = "test-support"))]
pub use store::MockCatalogStore;

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use buffa::Message as _;
use trogonai_catalog_proto::{CatalogSnapshot as ProtoCatalogSnapshot, ProviderSnapshot};

/// In-process cache of catalog + provider snapshots, refreshed from NATS KV.
#[derive(Clone)]
pub struct CatalogClient<S: CatalogStore> {
    store: S,
    config: CatalogClientConfig,
    cache: Arc<RwLock<ClientCache>>,
}

#[derive(Debug, Clone)]
struct ClientCache {
    catalog: Option<CatalogSnapshot>,
    providers: Option<Vec<String>>,
    catalog_fetched_at: Option<Instant>,
    providers_fetched_at: Option<Instant>,
}

impl ClientCache {
    fn new() -> Self {
        Self {
            catalog: None,
            providers: None,
            catalog_fetched_at: None,
            providers_fetched_at: None,
        }
    }
}

impl<S: CatalogStore> CatalogClient<S> {
    pub fn new(store: S, config: CatalogClientConfig) -> Self {
        Self {
            store,
            config,
            cache: Arc::new(RwLock::new(ClientCache::new())),
        }
    }

    /// Returns the cached catalog snapshot, refreshing from NATS when stale.
    pub async fn catalog_snapshot(&self) -> Result<CatalogSnapshot, CatalogClientError> {
        let ttl = Duration::from_secs(self.config.catalog_ttl_secs);
        let needs_refresh = self
            .cache
            .read()
            .map_err(|_| CatalogClientError::CachePoisoned)?
            .catalog_fetched_at
            .is_none_or(|t| t.elapsed() > ttl);

        if needs_refresh {
            self.refresh_catalog().await?;
        }

        self.cache
            .read()
            .map_err(|_| CatalogClientError::CachePoisoned)?
            .catalog
            .clone()
            .ok_or(CatalogClientError::CatalogUnavailable)
    }

    /// Returns callable provider names, refreshing from NATS when stale.
    pub async fn callable_providers(&self) -> Result<Vec<String>, CatalogClientError> {
        let ttl = Duration::from_secs(self.config.catalog_ttl_secs);
        let needs_refresh = self
            .cache
            .read()
            .map_err(|_| CatalogClientError::CachePoisoned)?
            .providers_fetched_at
            .is_none_or(|t| t.elapsed() > ttl);

        if needs_refresh {
            self.refresh_providers().await?;
        }

        self.cache
            .read()
            .map_err(|_| CatalogClientError::CachePoisoned)?
            .providers
            .clone()
            .ok_or(CatalogClientError::ProvidersUnavailable)
    }

    /// Synchronous read of the in-process cache (for sync builders like `build_config_options`).
    pub fn cached_snapshot(&self) -> Option<CatalogSnapshot> {
        self.cache.read().ok()?.catalog.clone()
    }

    /// Synchronous read of callable providers from cache.
    pub fn cached_providers(&self) -> Option<Vec<String>> {
        self.cache.read().ok()?.providers.clone()
    }

    /// Seed the in-process cache (used by embedded agents that refresh in background).
    pub fn seed_cache(&self, catalog: CatalogSnapshot, providers: Vec<String>) {
        if let Ok(mut cache) = self.cache.write() {
            let now = Instant::now();
            cache.catalog = Some(catalog);
            cache.providers = Some(providers);
            cache.catalog_fetched_at = Some(now);
            cache.providers_fetched_at = Some(now);
        }
    }

    async fn refresh_catalog(&self) -> Result<(), CatalogClientError> {
        let bytes = self
            .store
            .get(CATALOG_KEY)
            .await
            .map_err(|e| CatalogClientError::Store(e.to_string()))?;
        let Some(bytes) = bytes else {
            return Err(CatalogClientError::CatalogUnavailable);
        };
        let proto =
            ProtoCatalogSnapshot::decode_from_slice(&bytes).map_err(|e| CatalogClientError::Decode(e.to_string()))?;
        let snapshot = CatalogSnapshot::from_proto(&proto)?;
        if let Ok(mut cache) = self.cache.write() {
            cache.catalog = Some(snapshot);
            cache.catalog_fetched_at = Some(Instant::now());
        }
        Ok(())
    }

    async fn refresh_providers(&self) -> Result<(), CatalogClientError> {
        let bytes = self
            .store
            .get(PROVIDERS_KEY)
            .await
            .map_err(|e| CatalogClientError::Store(e.to_string()))?;
        let Some(bytes) = bytes else {
            return Err(CatalogClientError::ProvidersUnavailable);
        };
        let proto =
            ProviderSnapshot::decode_from_slice(&bytes).map_err(|e| CatalogClientError::Decode(e.to_string()))?;
        let providers: Vec<String> = proto.providers.into_iter().collect();
        if let Ok(mut cache) = self.cache.write() {
            cache.providers = Some(providers);
            cache.providers_fetched_at = Some(Instant::now());
        }
        Ok(())
    }
}

/// Open a catalog client backed by NATS JetStream KV.
pub async fn open(
    js: &async_nats::jetstream::Context,
    config: CatalogClientConfig,
) -> Result<CatalogClient<async_nats::jetstream::kv::Store>, CatalogClientError> {
    let store = provision(js).await?;
    Ok(CatalogClient::new(store, config))
}

#[cfg(test)]
mod tests;
