//! Per-session ZedToken and bulk permission result cache (ADR 0014).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moka::future::Cache;

pub const DEFAULT_ZEDTOKEN_CACHE_TTL_SECS: u64 = 5;
pub const DEFAULT_ZEDTOKEN_CACHE_MAX_ENTRIES: u64 = 10_000;
pub const ENV_ZEDTOKEN_CACHE_TTL_SECS: &str = "MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS";
pub const ENV_ZEDTOKEN_CACHE_TTL_MAX_SECS: &str = "MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_MAX_SECS";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKeyParams {
    pub session_id: String,
    pub principal: String,
    pub permission: String,
}

impl CacheKeyParams {
    #[must_use]
    pub fn new(session_id: impl Into<String>, principal: impl Into<String>, permission: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            principal: principal.into(),
            permission: permission.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceKey {
    pub resource_type: String,
    pub resource_id: String,
}

impl ResourceKey {
    #[must_use]
    pub fn new(resource_type: impl Into<String>, resource_id: impl Into<String>) -> Self {
        Self {
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ZedTokenCacheConfig {
    pub max_ttl: Duration,
    pub max_ttl_ceiling: Duration,
    pub max_entries: u64,
    pub enabled: bool,
}

impl Default for ZedTokenCacheConfig {
    fn default() -> Self {
        Self {
            max_ttl: Duration::from_secs(DEFAULT_ZEDTOKEN_CACHE_TTL_SECS),
            max_ttl_ceiling: Duration::from_secs(60),
            max_entries: DEFAULT_ZEDTOKEN_CACHE_MAX_ENTRIES,
            enabled: true,
        }
    }
}

impl ZedTokenCacheConfig {
    #[must_use]
    pub fn from_env<E: trogon_std::env::ReadEnv>(env: &E) -> Self {
        let mut cfg = Self::default();
        if let Ok(raw) = env.var(ENV_ZEDTOKEN_CACHE_TTL_SECS)
            && let Ok(secs) = raw.parse::<u64>()
        {
            cfg.max_ttl = Duration::from_secs(secs);
            cfg.enabled = secs > 0;
        }
        if let Ok(raw) = env.var(ENV_ZEDTOKEN_CACHE_TTL_MAX_SECS)
            && let Ok(secs) = raw.parse::<u64>()
        {
            cfg.max_ttl_ceiling = Duration::from_secs(secs);
        }
        cfg
    }

    #[must_use]
    pub fn entry_ttl(&self, zed_expiration: Option<Duration>) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }
        let configured = self.max_ttl.min(self.max_ttl_ceiling);
        match zed_expiration {
            Some(zed) if !zed.is_zero() => configured.min(zed),
            _ => configured,
        }
    }
}

#[derive(Clone, Debug)]
struct TimedValue<T> {
    value: T,
    inserted_at: Instant,
    ttl: Duration,
}

impl<T> TimedValue<T> {
    fn fresh(&self) -> bool {
        self.inserted_at.elapsed() < self.ttl
    }
}

#[derive(Debug, Default)]
pub struct ZedTokenCacheMetrics {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions_ttl: AtomicU64,
    pub evictions_invalidation: AtomicU64,
}

#[derive(Clone)]
pub struct ZedTokenCache {
    config: ZedTokenCacheConfig,
    zed_tokens: Cache<String, TimedValue<String>>,
    results: Cache<String, TimedValue<bool>>,
    metrics: Arc<ZedTokenCacheMetrics>,
}

impl std::fmt::Debug for ZedTokenCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZedTokenCache")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Default for ZedTokenCache {
    fn default() -> Self {
        Self::new(ZedTokenCacheConfig::default())
    }
}

impl ZedTokenCache {
    #[must_use]
    pub fn new(config: ZedTokenCacheConfig) -> Self {
        Self {
            zed_tokens: Cache::builder().max_capacity(config.max_entries).build(),
            results: Cache::builder().max_capacity(config.max_entries.saturating_mul(4)).build(),
            config,
            metrics: Arc::new(ZedTokenCacheMetrics::default()),
        }
    }

    #[must_use]
    pub fn metrics(&self) -> Arc<ZedTokenCacheMetrics> {
        self.metrics.clone()
    }

    #[must_use]
    pub fn config(&self) -> &ZedTokenCacheConfig {
        &self.config
    }

    #[must_use]
    pub fn zed_key(params: &CacheKeyParams) -> String {
        format!(
            "v1|zed|{}|{}|{}",
            params.session_id, params.principal, params.permission
        )
    }

    #[must_use]
    pub fn result_key(params: &CacheKeyParams, resource: &ResourceKey, checked_at_token: &str) -> String {
        format!(
            "v1|result|{}|{}|{}|{}|{}|{}",
            params.session_id,
            params.principal,
            params.permission,
            resource.resource_type,
            resource.resource_id,
            checked_at_token
        )
    }

    #[must_use]
    pub fn principal_key(subject_type: &str, subject_id: &str) -> String {
        format!("{subject_type}:{subject_id}")
    }

    pub async fn get_zed_token(&self, params: &CacheKeyParams) -> Option<String> {
        if !self.config.enabled {
            self.metrics.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let key = Self::zed_key(params);
        let Some(entry) = self.zed_tokens.get(&key).await else {
            self.metrics.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        if entry.fresh() {
            self.metrics.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.value)
        } else {
            self.zed_tokens.invalidate(&key).await;
            self.metrics.evictions_ttl.fetch_add(1, Ordering::Relaxed);
            self.metrics.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub async fn insert_zed_token(
        &self,
        params: &CacheKeyParams,
        token: String,
        zed_expiration: Option<Duration>,
    ) {
        if !self.config.enabled || token.trim().is_empty() {
            return;
        }
        let ttl = self.config.entry_ttl(zed_expiration);
        if ttl.is_zero() {
            return;
        }
        let key = Self::zed_key(params);
        self.zed_tokens
            .insert(
                key,
                TimedValue {
                    value: token,
                    inserted_at: Instant::now(),
                    ttl,
                },
            )
            .await;
    }

    pub async fn get_result(
        &self,
        params: &CacheKeyParams,
        resource: &ResourceKey,
        current_zed_token: Option<&str>,
    ) -> Option<bool> {
        if !self.config.enabled {
            self.metrics.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let checked_at = current_zed_token?;
        let key = Self::result_key(params, resource, checked_at);
        let Some(entry) = self.results.get(&key).await else {
            self.metrics.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        if entry.fresh() {
            self.metrics.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.value)
        } else {
            self.results.invalidate(&key).await;
            self.metrics.evictions_ttl.fetch_add(1, Ordering::Relaxed);
            self.metrics.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub async fn insert_results(
        &self,
        params: &CacheKeyParams,
        checked_at_token: &str,
        results: &[(ResourceKey, bool)],
        zed_expiration: Option<Duration>,
    ) {
        if !self.config.enabled || checked_at_token.trim().is_empty() {
            return;
        }
        let ttl = self.config.entry_ttl(zed_expiration);
        if ttl.is_zero() {
            return;
        }
        let inserted_at = Instant::now();
        for (resource, allowed) in results {
            let key = Self::result_key(params, resource, checked_at_token);
            self.results
                .insert(
                    key,
                    TimedValue {
                        value: *allowed,
                        inserted_at,
                        ttl,
                    },
                )
                .await;
        }
    }

    pub async fn invalidate_session(&self, session_id: &str) {
        let prefix_zed = format!("v1|zed|{session_id}|");
        let prefix_result = format!("v1|result|{session_id}|");
        self.invalidate_keys_with_prefix(&prefix_zed).await;
        self.invalidate_keys_with_prefix(&prefix_result).await;
        self.metrics
            .evictions_invalidation
            .fetch_add(1, Ordering::Relaxed);
    }

    pub async fn invalidate_server(&self, server_id: &str) {
        let needle = format!("|{server_id}|");
        self.invalidate_result_keys_containing(&needle).await;
        self.metrics
            .evictions_invalidation
            .fetch_add(1, Ordering::Relaxed);
    }

    async fn invalidate_keys_with_prefix(&self, prefix: &str) {
        let zed_keys: Vec<String> = self
            .zed_tokens
            .iter()
            .map(|(k, _)| k.as_ref().clone())
            .filter(|k| k.starts_with(prefix))
            .collect();
        for key in zed_keys {
            self.zed_tokens.invalidate(&key).await;
        }
        let result_keys: Vec<String> = self
            .results
            .iter()
            .map(|(k, _)| k.as_ref().clone())
            .filter(|k| k.starts_with(prefix))
            .collect();
        for key in result_keys {
            self.results.invalidate(&key).await;
        }
    }

    async fn invalidate_result_keys_containing(&self, needle: &str) {
        let result_keys: Vec<String> = self
            .results
            .iter()
            .map(|(k, _)| k.as_ref().clone())
            .filter(|k| k.contains(needle))
            .collect();
        for key in result_keys {
            self.results.invalidate(&key).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_params(session: &str, principal: &str) -> CacheKeyParams {
        CacheKeyParams::new(session, principal, "invoke")
    }

    fn sample_resource(server: &str, tool: &str) -> ResourceKey {
        ResourceKey::new("trogon/mcp_tool", format!("{server}|{tool}"))
    }

    #[test]
    fn zed_key_includes_session_principal_permission() {
        let params = sample_params("sess-1", "trogon/principal:alice");
        assert_eq!(
            ZedTokenCache::zed_key(&params),
            "v1|zed|sess-1|trogon/principal:alice|invoke"
        );
    }

    #[test]
    fn result_key_includes_checked_at_token() {
        let params = sample_params("sess-1", "trogon/principal:alice");
        let resource = sample_resource("filesystem", "read_file");
        assert_eq!(
            ZedTokenCache::result_key(&params, &resource, "tok-abc"),
            "v1|result|sess-1|trogon/principal:alice|invoke|trogon/mcp_tool|filesystem|read_file|tok-abc"
        );
    }

    #[test]
    fn principal_key_is_unique_per_subject() {
        assert_eq!(
            ZedTokenCache::principal_key("trogon/principal", "alice"),
            "trogon/principal:alice"
        );
        assert_ne!(
            ZedTokenCache::principal_key("trogon/principal", "alice"),
            ZedTokenCache::principal_key("trogon/principal", "bob")
        );
    }

    #[test]
    fn entry_ttl_clamps_to_configured_max() {
        let mut cfg = ZedTokenCacheConfig::default();
        cfg.max_ttl = Duration::from_secs(5);
        cfg.max_ttl_ceiling = Duration::from_secs(60);
        assert_eq!(cfg.entry_ttl(Some(Duration::from_secs(120))), Duration::from_secs(5));
        assert_eq!(cfg.entry_ttl(Some(Duration::from_secs(2))), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn different_principals_do_not_share_zedtoken_entries() {
        let cache = ZedTokenCache::default();
        let params_a = sample_params("sess-shared", "trogon/principal:alice");
        let params_b = sample_params("sess-shared", "trogon/principal:bob");

        cache
            .insert_zed_token(&params_a, "token-a".into(), None)
            .await;

        assert_eq!(cache.get_zed_token(&params_a).await.as_deref(), Some("token-a"));
        assert!(cache.get_zed_token(&params_b).await.is_none());
    }

    #[tokio::test]
    async fn different_principals_do_not_share_permission_results() {
        let cache = ZedTokenCache::default();
        let params_a = sample_params("sess-shared", "trogon/principal:alice");
        let params_b = sample_params("sess-shared", "trogon/principal:bob");
        let resource = sample_resource("filesystem", "deploy");

        cache
            .insert_zed_token(&params_a, "tok-shared".into(), None)
            .await;
        cache
            .insert_results(&params_a, "tok-shared", &[(resource.clone(), true)], None)
            .await;

        assert_eq!(
            cache
                .get_result(&params_a, &resource, Some("tok-shared"))
                .await,
            Some(true)
        );
        assert!(
            cache
                .get_result(&params_b, &resource, Some("tok-shared"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn zedtoken_passes_through_on_subsequent_lookup() {
        let cache = ZedTokenCache::default();
        let params = sample_params("sess-1", "trogon/principal:alice");

        cache
            .insert_zed_token(&params, "tok-first".into(), None)
            .await;
        assert_eq!(cache.get_zed_token(&params).await.as_deref(), Some("tok-first"));

        cache
            .insert_zed_token(&params, "tok-second".into(), None)
            .await;
        assert_eq!(cache.get_zed_token(&params).await.as_deref(), Some("tok-second"));
    }

    #[tokio::test]
    async fn ttl_expiry_evicts_zedtoken_entry() {
        let mut cfg = ZedTokenCacheConfig::default();
        cfg.max_ttl = Duration::from_millis(1);
        let cache = ZedTokenCache::new(cfg);
        let params = sample_params("sess-ttl", "trogon/principal:alice");

        cache
            .insert_zed_token(&params, "tok-expire".into(), None)
            .await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(cache.get_zed_token(&params).await.is_none());
        assert!(cache.metrics().evictions_ttl.load(Ordering::Relaxed) >= 1);
    }

    #[tokio::test]
    async fn session_invalidation_drops_entries() {
        let cache = ZedTokenCache::default();
        let params = sample_params("sess-drop", "trogon/principal:alice");
        let resource = sample_resource("srv", "tool");

        cache
            .insert_zed_token(&params, "tok-drop".into(), None)
            .await;
        cache
            .insert_results(&params, "tok-drop", &[(resource.clone(), false)], None)
            .await;

        cache.invalidate_session("sess-drop").await;

        assert!(cache.get_zed_token(&params).await.is_none());
        assert!(
            cache
                .get_result(&params, &resource, Some("tok-drop"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn server_invalidation_scopes_to_matching_resources() {
        let cache = ZedTokenCache::default();
        let params_a = sample_params("sess-a", "trogon/principal:alice");
        let params_b = sample_params("sess-b", "trogon/principal:alice");
        let resource_a = sample_resource("server-a", "tool");
        let resource_b = sample_resource("server-b", "tool");

        cache
            .insert_zed_token(&params_a, "tok-a".into(), None)
            .await;
        cache
            .insert_results(&params_a, "tok-a", &[(resource_a.clone(), true)], None)
            .await;
        cache
            .insert_zed_token(&params_b, "tok-b".into(), None)
            .await;
        cache
            .insert_results(&params_b, "tok-b", &[(resource_b.clone(), true)], None)
            .await;

        cache.invalidate_server("server-a").await;

        assert!(
            cache
                .get_result(&params_a, &resource_a, Some("tok-a"))
                .await
                .is_none()
        );
        assert_eq!(
            cache
                .get_result(&params_b, &resource_b, Some("tok-b"))
                .await,
            Some(true)
        );
    }
}
