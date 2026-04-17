use async_nats::jetstream::{self, kv};
use futures_util::StreamExt as _;

use crate::models::session::{ConsoleSession, RawSession};

pub const SESSIONS_BUCKET: &str = "SESSIONS";

#[derive(Clone)]
pub struct SessionReader {
    kv: kv::Store,
}

impl SessionReader {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        // Open existing SESSIONS bucket (created by trogon-agent). Read-only usage.
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SESSIONS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(Self { kv })
    }

    pub async fn list(&self) -> Result<Vec<ConsoleSession>, String> {
        let mut keys = self.kv.keys().await.map_err(|e| e.to_string())?;
        let mut sessions = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if let Some(bytes) = self.kv.get(&key).await.map_err(|e| e.to_string())? {
                if let Ok(raw) = serde_json::from_slice::<RawSession>(&bytes) {
                    sessions.push(ConsoleSession::from(raw));
                }
            }
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    pub async fn list_by_tenant(&self, tenant_id: &str) -> Result<Vec<ConsoleSession>, String> {
        let all = self.list().await?;
        Ok(all.into_iter().filter(|s| s.tenant_id == tenant_id).collect())
    }

    pub async fn get(&self, tenant_id: &str, session_id: &str) -> Result<Option<ConsoleSession>, String> {
        let key = format!("{tenant_id}.{session_id}");
        match self.kv.get(&key).await.map_err(|e| e.to_string())? {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<RawSession>(&bytes)
                .map(|r| Some(ConsoleSession::from(r)))
                .map_err(|e| e.to_string()),
        }
    }
}
