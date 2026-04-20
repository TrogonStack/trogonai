use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;

use crate::models::agent::{AgentDefinition, AgentVersion};
use crate::store::traits::AgentRepository;

pub const AGENTS_BUCKET: &str = "CONSOLE_AGENTS";
pub const AGENT_VERSIONS_BUCKET: &str = "CONSOLE_AGENT_VERSIONS";

#[derive(Clone)]
pub struct AgentStore {
    kv: kv::Store,
    versions_kv: kv::Store,
}

impl AgentStore {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: AGENTS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let versions_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: AGENT_VERSIONS_BUCKET.to_string(),
                history: 20,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self { kv, versions_kv })
    }

    pub async fn list(&self) -> Result<Vec<AgentDefinition>, String> {
        let mut keys = self.kv.keys().await.map_err(|e| e.to_string())?;
        let mut agents = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if let Some(agent) = self.get(&key).await? {
                agents.push(agent);
            }
        }
        agents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(agents)
    }

    pub async fn get(&self, id: &str) -> Result<Option<AgentDefinition>, String> {
        match self.kv.get(id).await.map_err(|e| e.to_string())? {
            None => Ok(None),
            Some(bytes) => {
                serde_json::from_slice::<AgentDefinition>(&bytes)
                    .map(Some)
                    .map_err(|e| e.to_string())
            }
        }
    }

    pub async fn put(&self, agent: &AgentDefinition) -> Result<(), String> {
        let bytes = serde_json::to_vec(agent).expect("AgentDefinition serialization");
        self.kv
            .put(&agent.id, Bytes::from(bytes))
            .await
            .map_err(|e| e.to_string())?;

        // Record version snapshot
        let version_key = format!("{}.v{}", agent.id, agent.version);
        let version = AgentVersion {
            version: agent.version,
            updated_at: agent.updated_at.clone(),
            model_id: agent.model.id.clone(),
        };
        let vbytes = serde_json::to_vec(&version).expect("AgentVersion serialization");
        self.versions_kv
            .put(&version_key, Bytes::from(vbytes))
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<(), String> {
        self.kv.delete(id).await.map_err(|e| e.to_string())
    }

    pub async fn list_versions(&self, agent_id: &str) -> Result<Vec<AgentVersion>, String> {
        let prefix = format!("{agent_id}.v");
        let mut keys = self.versions_kv.keys().await.map_err(|e| e.to_string())?;
        let mut versions = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if !key.starts_with(&prefix) {
                continue;
            }
            if let Some(bytes) = self.versions_kv.get(&key).await.map_err(|e| e.to_string())? {
                if let Ok(v) = serde_json::from_slice::<AgentVersion>(&bytes) {
                    versions.push(v);
                }
            }
        }
        versions.sort_by_key(|v| v.version);
        Ok(versions)
    }
}

impl AgentRepository for AgentStore {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<AgentDefinition>, String>> + Send + '_>> {
        Box::pin(async move { self.list().await })
    }
    fn get<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<AgentDefinition>, String>> + Send + 'a>> {
        Box::pin(async move { self.get(id).await })
    }
    fn put<'a>(&'a self, agent: &'a AgentDefinition) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.put(agent).await })
    }
    fn delete<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.delete(id).await })
    }
    fn list_versions<'a>(&'a self, agent_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<AgentVersion>, String>> + Send + 'a>> {
        Box::pin(async move { self.list_versions(agent_id).await })
    }
}
