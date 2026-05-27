use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use moka::future::Cache;

use crate::error::StsError;
use crate::registry::{AgentRegistryRecord, RegistryLookup, RegistryLookupRequest, RegistryLookupResponse};

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
    bundle_pem: Arc<tokio::sync::RwLock<String>>,
}

impl TrustBundleCache {
    pub fn from_pem(pem: String) -> Self {
        Self {
            bundle_pem: Arc::new(tokio::sync::RwLock::new(pem)),
        }
    }

    pub async fn pem(&self) -> String {
        self.bundle_pem.read().await.clone()
    }

    pub async fn reload_from_file(&self, path: &str) -> Result<(), StsError> {
        let pem = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| StsError::ServerError(format!("read trust bundle {path}: {e}")))?;
        *self.bundle_pem.write().await = pem;
        Ok(())
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
