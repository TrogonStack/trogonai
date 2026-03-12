//! NATS KV store for incident state.
//!
//! Maintains the current state of every incident received via webhook.
//! Each KV key is the incident ID; the value is the full incident JSON
//! from the webhook payload.  This lets the agent (or any consumer) read
//! the latest known state of an incident without hitting the incident.io API.
//!
//! The underlying JetStream KV bucket (`INCIDENTS`) keeps 10 revisions per key,
//! so recent history is preserved for auditing.

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt;

pub const BUCKET: &str = "INCIDENTS";

/// Error from the incident store.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// NATS KV-backed store for incident state.
#[derive(Clone)]
pub struct IncidentStore {
    kv: kv::Store,
}

impl IncidentStore {
    /// Open (or create) the `INCIDENTS` KV bucket.
    pub async fn open(js: &jetstream::Context) -> Result<Self, StoreError> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: BUCKET.to_string(),
                history: 10,
                ..Default::default()
            })
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        Ok(Self { kv })
    }

    /// Upsert the current state of an incident (keyed by incident ID).
    pub async fn upsert(&self, incident_id: &str, incident_json: &[u8]) -> Result<(), StoreError> {
        self.kv
            .put(incident_id, Bytes::copy_from_slice(incident_json))
            .await
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(())
    }

    /// Retrieve the stored state of an incident by ID.
    ///
    /// Returns `None` when the incident has not been seen yet.
    pub async fn get(&self, incident_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self
            .kv
            .get(incident_id)
            .await
            .map_err(|e| StoreError(e.to_string()))?
        {
            None => Ok(None),
            Some(bytes) => Ok(Some(bytes.to_vec())),
        }
    }

    /// List all stored incidents.
    ///
    /// Iterates over all keys in the KV bucket and returns the latest value for
    /// each incident.  Keys with no value (deleted entries) are skipped.
    pub async fn list(&self) -> Result<Vec<Vec<u8>>, StoreError> {
        let mut keys = self
            .kv
            .keys()
            .await
            .map_err(|e| StoreError(e.to_string()))?;

        let mut results = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| StoreError(e.to_string()))?;
            if let Some(bytes) = self
                .kv
                .get(&key)
                .await
                .map_err(|e| StoreError(e.to_string()))?
            {
                results.push(bytes.to_vec());
            }
        }
        Ok(results)
    }
}
