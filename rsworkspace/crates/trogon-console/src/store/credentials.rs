use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;

use crate::models::credential::{Credential, CredentialVault};
use crate::store::traits::CredentialRepository;

pub const CREDS_BUCKET: &str = "CONSOLE_CREDS";
pub const VAULTS_BUCKET: &str = "CONSOLE_VAULTS";

#[derive(Clone)]
pub struct CredentialStore {
    creds_kv: kv::Store,
    vaults_kv: kv::Store,
}

impl CredentialStore {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        let creds_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: CREDS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let vaults_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: VAULTS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self {
            creds_kv,
            vaults_kv,
        })
    }

    // Vault operations
    pub async fn get_or_create_vault(&self, env_id: &str) -> Result<CredentialVault, String> {
        if let Some(bytes) = self
            .vaults_kv
            .get(env_id)
            .await
            .map_err(|e| e.to_string())?
        {
            return serde_json::from_slice::<CredentialVault>(&bytes).map_err(|e| e.to_string());
        }
        let vault = CredentialVault {
            id: format!("vlt_{}", uuid::Uuid::new_v4().simple()),
            env_id: env_id.to_string(),
            created_at: chrono_now(),
        };
        let bytes = serde_json::to_vec(&vault).expect("CredentialVault serialization");
        self.vaults_kv
            .put(env_id, Bytes::from(bytes))
            .await
            .map_err(|e| e.to_string())?;
        Ok(vault)
    }

    pub async fn get_vault(&self, env_id: &str) -> Result<Option<CredentialVault>, String> {
        match self
            .vaults_kv
            .get(env_id)
            .await
            .map_err(|e| e.to_string())?
        {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<CredentialVault>(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
        }
    }

    // Credential operations — key: "{env_id}.{cred_id}"
    pub async fn list(&self, env_id: &str) -> Result<Vec<Credential>, String> {
        let prefix = format!("{env_id}.");
        let mut keys = self.creds_kv.keys().await.map_err(|e| e.to_string())?;
        let mut creds = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if !key.starts_with(&prefix) {
                continue;
            }
            if let Some(bytes) = self.creds_kv.get(&key).await.map_err(|e| e.to_string())?
                && let Ok(c) = serde_json::from_slice::<Credential>(&bytes)
            {
                creds.push(c);
            }
        }
        creds.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(creds)
    }

    pub async fn get(&self, env_id: &str, cred_id: &str) -> Result<Option<Credential>, String> {
        let key = format!("{env_id}.{cred_id}");
        match self.creds_kv.get(&key).await.map_err(|e| e.to_string())? {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<Credential>(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
        }
    }

    pub async fn put(&self, cred: &Credential) -> Result<(), String> {
        let key = format!("{}.{}", cred.env_id, cred.id);
        let bytes = serde_json::to_vec(cred).expect("Credential serialization");
        self.creds_kv
            .put(&key, Bytes::from(bytes))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete(&self, env_id: &str, cred_id: &str) -> Result<(), String> {
        let key = format!("{env_id}.{cred_id}");
        self.creds_kv.delete(&key).await.map_err(|e| e.to_string())
    }
}

impl CredentialRepository for CredentialStore {
    fn get_or_create_vault<'a>(
        &'a self,
        env_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CredentialVault, String>> + Send + 'a>,
    > {
        Box::pin(async move { self.get_or_create_vault(env_id).await })
    }
    fn get_vault<'a>(
        &'a self,
        env_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<CredentialVault>, String>> + Send + 'a>,
    > {
        Box::pin(async move { self.get_vault(env_id).await })
    }
    fn list<'a>(
        &'a self,
        env_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<Credential>, String>> + Send + 'a>,
    > {
        Box::pin(async move { self.list(env_id).await })
    }
    fn get<'a>(
        &'a self,
        env_id: &'a str,
        cred_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Credential>, String>> + Send + 'a>,
    > {
        Box::pin(async move { self.get(env_id, cred_id).await })
    }
    fn put<'a>(
        &'a self,
        cred: &'a Credential,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.put(cred).await })
    }
    fn delete<'a>(
        &'a self,
        env_id: &'a str,
        cred_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.delete(env_id, cred_id).await })
    }
}

fn chrono_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}", d.as_secs()))
        .unwrap_or_else(|_| "0".to_string())
}
