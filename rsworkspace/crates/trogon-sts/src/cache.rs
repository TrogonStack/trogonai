use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use moka::future::Cache;
use tracing::warn;

use crate::error::StsError;
use crate::registry::{AgentRegistryRecord, RegistryLookup, RegistryLookupRequest, RegistryLookupResponse};
use crate::svid_verify::verify_leaf_against_bundle;
use crate::workload_svid::WorkloadSvid;

const REGISTRY_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct JwksCache {
    bootstrap: Arc<JwksStore>,
    mesh: Arc<JwksStore>,
}

#[derive(Clone)]
struct JwksStore {
    keys: Arc<tokio::sync::RwLock<JwkSet>>,
}

impl JwksStore {
    fn new(initial: JwkSet) -> Self {
        Self {
            keys: Arc::new(tokio::sync::RwLock::new(initial)),
        }
    }

    async fn get(&self) -> JwkSet {
        self.keys.read().await.clone()
    }

    async fn replace(&self, set: JwkSet) {
        *self.keys.write().await = set;
    }
}

impl JwksCache {
    pub fn new(bootstrap: JwkSet, mesh: JwkSet) -> Self {
        Self {
            bootstrap: Arc::new(JwksStore::new(bootstrap)),
            mesh: Arc::new(JwksStore::new(mesh)),
        }
    }

    pub async fn bootstrap_jwks(&self) -> JwkSet {
        self.bootstrap.get().await
    }

    pub async fn mesh_jwks(&self) -> JwkSet {
        self.mesh.get().await
    }

    pub async fn update_bootstrap(&self, set: JwkSet) {
        self.bootstrap.replace(set).await;
    }

    pub async fn update_mesh(&self, set: JwkSet) {
        self.mesh.replace(set).await;
    }
}

#[derive(Clone)]
pub struct TrustBundleCache {
    bundles: Arc<tokio::sync::RwLock<HashMap<String, String>>>,
    default_domain: Arc<str>,
}

impl TrustBundleCache {
    pub fn from_pem(pem: String) -> Self {
        Self::from_pem_for_domain("local", pem)
    }

    pub fn from_pem_for_domain(trust_domain: impl Into<String>, pem: String) -> Self {
        let domain = trust_domain.into();
        let mut map = HashMap::new();
        map.insert(domain.clone(), pem);
        Self {
            bundles: Arc::new(tokio::sync::RwLock::new(map)),
            default_domain: Arc::from(domain.as_str()),
        }
    }

    pub async fn pem(&self) -> Result<String, StsError> {
        self.pem_for_domain(&self.default_domain).await
    }

    pub async fn pem_for_domain(&self, trust_domain: &str) -> Result<String, StsError> {
        let bundles = self.bundles.read().await;
        bundles
            .get(trust_domain)
            .cloned()
            .ok_or_else(|| {
                StsError::ServerError(format!(
                    "no trust bundle for trust domain {trust_domain}"
                ))
            })
    }

    pub async fn upsert_domain(&self, trust_domain: String, pem: String) {
        self.bundles.write().await.insert(trust_domain, pem);
    }

    pub async fn verify(&self, svid: &WorkloadSvid) -> Result<(), StsError> {
        if svid.leaf_der.is_empty() {
            return Ok(());
        }
        let bundle = self.pem_for_domain(svid.spiffe_id.trust_domain()).await?;
        verify_leaf_against_bundle(&svid.leaf_der, &bundle)
    }

    pub async fn reload_from_file(&self, path: &str) -> Result<(), StsError> {
        let pem = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| StsError::ServerError(format!("read trust bundle {path}: {e}")))?;
        self.upsert_domain(self.default_domain.to_string(), pem).await;
        Ok(())
    }
}

pub async fn load_trust_bundle_from_kv(
    nats: &async_nats::Client,
    bucket: &str,
    key: &str,
) -> Result<(String, String), StsError> {
    let js = async_nats::jetstream::new(nats.clone());
    let store = js
        .get_key_value(bucket)
        .await
        .map_err(|e| StsError::ServerError(format!("kv bucket {bucket}: {e}")))?;
    let value = store
        .get(key)
        .await
        .map_err(|e| StsError::ServerError(format!("kv get {key}: {e}")))?
        .ok_or_else(|| StsError::ServerError(format!("kv key {key} missing")))?;
    let pem = String::from_utf8(value.to_vec())
        .map_err(|e| StsError::ServerError(format!("trust bundle utf8: {e}")))?;
    let trust_domain = key
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(key)
        .to_string();
    Ok((trust_domain, pem))
}

pub fn spawn_trust_bundle_watch(
    nats: async_nats::Client,
    cache: TrustBundleCache,
    bucket: &'static str,
) {
    tokio::spawn(async move {
        loop {
            match watch_trust_bundles(&nats, cache.clone(), bucket).await {
                Ok(()) => warn!("trust bundle KV watch ended; restarting"),
                Err(e) => {
                    warn!(error = %e, "trust bundle KV watch failed; retrying in 5s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

async fn watch_trust_bundles(
    nats: &async_nats::Client,
    cache: TrustBundleCache,
    bucket: &str,
) -> Result<(), StsError> {
    use futures::StreamExt;

    let js = async_nats::jetstream::new(nats.clone());
    let store = js
        .get_key_value(bucket)
        .await
        .map_err(|e| StsError::ServerError(format!("kv bucket {bucket}: {e}")))?;
    let mut watcher = store
        .watch(">")
        .await
        .map_err(|e| StsError::ServerError(format!("kv watch {bucket}: {e}")))?;
    while let Some(entry) = watcher.next().await {
        let entry = entry.map_err(|e| StsError::ServerError(format!("kv entry: {e}")))?;
        if entry.operation == async_nats::jetstream::kv::Operation::Delete {
            continue;
        }
        let pem = String::from_utf8(entry.value.to_vec())
            .map_err(|e| StsError::ServerError(format!("trust bundle utf8: {e}")))?;
        let trust_domain = entry
            .key
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(entry.key.as_str())
            .to_string();
        cache.upsert_domain(trust_domain, pem).await;
    }
    Ok(())
}

#[cfg(test)]
mod trust_bundle_tests {
    use super::*;

    #[tokio::test]
    async fn verify_skips_unattested_svid_without_der() {
        use crate::spiffe_id::SpiffeId;

        let cache = TrustBundleCache::from_pem(String::new());
        let svid = WorkloadSvid::new(
            SpiffeId::parse("spiffe://acme.local/ns/x").expect("id"),
            Vec::new(),
            String::new(),
        );
        cache.verify(&svid).await.expect("noop verify");
    }

    #[tokio::test]
    async fn upsert_and_read_by_domain() {
        let cache = TrustBundleCache::from_pem_for_domain("acme.local", "ca-pem".into());
        cache
            .upsert_domain("other.local".into(), "other-pem".into())
            .await;
        assert_eq!(
            cache.pem_for_domain("other.local").await.expect("get"),
            "other-pem"
        );
    }
}

#[derive(Clone)]
pub struct RegistryCache<R: RegistryLookup> {
    inner: R,
    cache: Cache<String, AgentRegistryRecord>,
}

impl<R: RegistryLookup + Clone> RegistryCache<R> {
    pub fn new(inner: R) -> Self {
        let cache = Cache::builder()
            .time_to_live(REGISTRY_CACHE_TTL)
            .max_capacity(10_000)
            .build();
        Self { inner, cache }
    }

    pub fn inner(&self) -> &R {
        &self.inner
    }

    pub async fn lookup(&self, agent_id: &str) -> Result<AgentRegistryRecord, StsError> {
        if let Some(record) = self.cache.get(agent_id).await {
            return Ok(record);
        }
        let response = self
            .inner
            .lookup(&RegistryLookupRequest {
                agent_id: agent_id.to_string(),
                version: None,
            })
            .await?;
        self.cache.insert(agent_id.to_string(), response.clone()).await;
        Ok(response)
    }

    pub async fn lookup_raw(&self, request: &RegistryLookupRequest) -> Result<RegistryLookupResponse, StsError> {
        self.inner.lookup_raw(request).await
    }
}

#[async_trait]
pub trait JwksSource: Send + Sync {
    async fn fetch_bootstrap_jwks(&self) -> Result<JwkSet, StsError>;
    async fn fetch_mesh_jwks(&self) -> Result<JwkSet, StsError>;
}

pub struct StaticJwksSource {
    bootstrap: JwkSet,
    mesh: JwkSet,
}

impl StaticJwksSource {
    pub fn new(bootstrap: JwkSet, mesh: JwkSet) -> Self {
        Self { bootstrap, mesh }
    }
}

#[async_trait]
impl JwksSource for StaticJwksSource {
    async fn fetch_bootstrap_jwks(&self) -> Result<JwkSet, StsError> {
        Ok(self.bootstrap.clone())
    }

    async fn fetch_mesh_jwks(&self) -> Result<JwkSet, StsError> {
        Ok(self.mesh.clone())
    }
}

pub struct HttpJwksSource {
    bootstrap_url: String,
    http: reqwest::Client,
}

impl HttpJwksSource {
    pub fn new(bootstrap_url: String) -> Result<Self, StsError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| StsError::ServerError(format!("jwks http client: {e}")))?;
        Ok(Self { bootstrap_url, http })
    }

    async fn fetch_url(&self, url: &str) -> Result<JwkSet, StsError> {
        let body = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| StsError::ServerError(format!("jwks fetch {url}: {e}")))?
            .error_for_status()
            .map_err(|e| StsError::ServerError(format!("jwks HTTP {url}: {e}")))?
            .text()
            .await
            .map_err(|e| StsError::ServerError(format!("jwks body {url}: {e}")))?;
        serde_json::from_str(&body).map_err(|e| StsError::ServerError(format!("jwks json {url}: {e}")))
    }
}

#[async_trait]
impl JwksSource for HttpJwksSource {
    async fn fetch_bootstrap_jwks(&self) -> Result<JwkSet, StsError> {
        self.fetch_url(&self.bootstrap_url).await
    }

    async fn fetch_mesh_jwks(&self) -> Result<JwkSet, StsError> {
        self.fetch_url(&self.bootstrap_url).await
    }
}
