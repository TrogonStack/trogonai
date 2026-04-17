use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;

use crate::models::environment::Environment;
use crate::store::traits::EnvironmentRepository;

pub const ENVS_BUCKET: &str = "CONSOLE_ENVS";

#[derive(Clone)]
pub struct EnvironmentStore {
    kv: kv::Store,
}

impl EnvironmentStore {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: ENVS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { kv })
    }

    pub async fn list(&self) -> Result<Vec<Environment>, String> {
        let mut keys = self.kv.keys().await.map_err(|e| e.to_string())?;
        let mut envs = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if let Some(env) = self.get(&key).await? {
                envs.push(env);
            }
        }
        envs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(envs)
    }

    pub async fn get(&self, id: &str) -> Result<Option<Environment>, String> {
        match self.kv.get(id).await.map_err(|e| e.to_string())? {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<Environment>(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
        }
    }

    pub async fn put(&self, env: &Environment) -> Result<(), String> {
        let bytes = serde_json::to_vec(env).map_err(|e| e.to_string())?;
        self.kv
            .put(&env.id, Bytes::from(bytes))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<(), String> {
        self.kv.delete(id).await.map_err(|e| e.to_string())
    }
}

impl EnvironmentRepository for EnvironmentStore {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Environment>, String>> + Send + '_>> {
        Box::pin(async move { self.list().await })
    }
    fn get<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<Environment>, String>> + Send + 'a>> {
        Box::pin(async move { self.get(id).await })
    }
    fn put<'a>(&'a self, env: &'a Environment) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.put(env).await })
    }
    fn delete<'a>(&'a self, id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.delete(id).await })
    }
}
