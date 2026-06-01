use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tracing::{debug, info};
use trogon_sts_client::MintedMeshToken;

use crate::egress::config::EgressMintConfig;

pub const MCP_SESSION_HEADER: &str = "mcp-session-id";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKeyParts {
    pub tenant: String,
    pub caller_sub: String,
    pub target_aud: String,
    pub session_id: String,
    pub scope_fingerprint: String,
}

impl CacheKeyParts {
    pub fn cache_key(&self) -> String {
        format!(
            "mesh_cache:{}:{}:{}:{}:{}",
            self.tenant, self.caller_sub, self.target_aud, self.session_id, self.scope_fingerprint
        )
    }
}

#[derive(Clone, Debug)]
struct CachedMeshEntry {
    token: MintedMeshToken,
    serve_until: i64,
    refresh_at: i64,
}

#[derive(Clone)]
pub struct MeshEgressCache {
    config: EgressMintConfig,
    entries: moka::future::Cache<String, CachedMeshEntry>,
}

impl MeshEgressCache {
    pub fn new(config: EgressMintConfig) -> Self {
        let entries = moka::future::Cache::builder()
            .max_capacity(config.cache_max_entries)
            .build();
        Self { config, entries }
    }

    pub async fn lookup(&self, key: &CacheKeyParts) -> Option<MintedMeshToken> {
        let cache_key = key.cache_key();
        let Some(entry) = self.entries.get(&cache_key).await else {
            debug!(event = "mesh_cache_miss", cache_key = %cache_key, "mesh egress cache miss");
            return None;
        };
        let now = now_unix();
        if now >= entry.serve_until {
            self.entries.invalidate(&cache_key).await;
            info!(event = "mesh_cache_eviction", cache_key = %cache_key, reason = "expired");
            return None;
        }
        debug!(event = "mesh_cache_hit", cache_key = %cache_key, "mesh egress cache hit");
        if now >= entry.refresh_at {
            debug!(event = "mesh_cache_refresh_due", cache_key = %cache_key, "mesh egress cache refresh due");
        }
        Some(entry.token.clone())
    }

    pub async fn store(&self, key: &CacheKeyParts, token: MintedMeshToken) {
        let now = now_unix();
        let ttl = cache_entry_ttl(token.exp, now, self.config.mesh_token_ttl_secs, self.config.clock_skew_secs);
        let serve_until = token.exp - self.config.clock_skew_secs;
        let refresh_at = now + i64::try_from(ttl.as_secs() / 2).unwrap_or(0);
        let entry = CachedMeshEntry {
            token,
            serve_until,
            refresh_at,
        };
        self.entries.insert(key.cache_key(), entry).await;
    }

    pub fn needs_proactive_refresh(&self, _key: &CacheKeyParts, token: &MintedMeshToken) -> bool {
        let now = now_unix();
        let remaining = token.exp - self.config.clock_skew_secs - now;
        let threshold = i64::try_from(self.config.proactive_refresh_threshold().as_secs()).unwrap_or(0);
        remaining <= threshold
    }
}

pub fn scope_fingerprint(scope: Option<&str>) -> String {
    let Some(raw) = scope.filter(|s| !s.trim().is_empty()) else {
        return "*".to_string();
    };
    let tokens: BTreeSet<&str> = raw.split_whitespace().collect();
    if tokens.is_empty() {
        return "*".to_string();
    }
    let joined = tokens.iter().copied().collect::<Vec<_>>().join(" ");
    let digest = Sha256::digest(joined.as_bytes());
    hex::encode(digest)
}

pub fn tool_scope(server_id: &str, tool_name: &str) -> String {
    format!("tool:{server_id}::{tool_name}")
}

pub fn cache_entry_ttl(exp: i64, now: i64, mesh_token_ttl_secs: u64, clock_skew_secs: i64) -> Duration {
    let skew_leeway = clock_skew_secs;
    let remaining = exp - now - skew_leeway;
    if remaining <= 0 {
        return Duration::ZERO;
    }
    let cap = mesh_token_ttl_secs / 2;
    let ttl_secs = (remaining).min(i64::try_from(cap).unwrap_or(cap as i64));
    Duration::from_secs(u64::try_from(ttl_secs).unwrap_or(0))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_fingerprint_absent_is_star() {
        assert_eq!(scope_fingerprint(None), "*");
        assert_eq!(scope_fingerprint(Some("")), "*");
    }

    #[test]
    fn scope_fingerprint_sorts_tokens() {
        let a = scope_fingerprint(Some("tool:b::x tool:a::y"));
        let b = scope_fingerprint(Some("tool:a::y tool:b::x"));
        assert_eq!(a, b);
        assert_ne!(a, "*");
    }

    #[test]
    fn cache_entry_ttl_respects_cap_and_skew() {
        let now = 1_000_000;
        let exp = now + 120;
        let ttl = cache_entry_ttl(exp, now, 120, 30);
        assert_eq!(ttl, Duration::from_secs(60));
    }

    #[test]
    fn cache_entry_ttl_zero_past_exp_minus_skew() {
        let now = 1_000_000;
        let exp = now + 20;
        let ttl = cache_entry_ttl(exp, now, 120, 30);
        assert_eq!(ttl, Duration::ZERO);
    }

    #[test]
    fn cache_key_includes_target_aud_for_callbacks() {
        let backend = CacheKeyParts {
            tenant: "acme".into(),
            caller_sub: "user:alice".into(),
            target_aud: "urn:trogon:mcp:backend:acme:github".into(),
            session_id: "sess".into(),
            scope_fingerprint: "*".into(),
        };
        let client = CacheKeyParts {
            target_aud: "urn:trogon:mcp:client:acme:desktop".into(),
            ..backend.clone()
        };
        assert_ne!(backend.cache_key(), client.cache_key());
    }

    #[tokio::test]
    async fn serve_until_boundary_at_exp_minus_30s() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let config = EgressMintConfig {
            mesh_token_ttl_secs: 120,
            cache_max_entries: 100,
            actor_token: "spiffe://test".into(),
            clock_skew_secs: 30,
            prefix: "mcp".into(),
        };
        let cache = MeshEgressCache::new(config);
        let key = CacheKeyParts {
            tenant: "acme".into(),
            caller_sub: "user:alice".into(),
            target_aud: "urn:trogon:mcp:backend:acme:github".into(),
            session_id: "sess-1".into(),
            scope_fingerprint: "*".into(),
        };
        let token = MintedMeshToken {
            access_token: "tok".into(),
            expires_in: 120,
            exp: now + 120,
            iss: "urn:trogon:sts:mesh".into(),
            kid: None,
        };
        cache.store(&key, token.clone()).await;
        assert!(cache.lookup(&key).await.is_some());
    }
}
