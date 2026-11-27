use std::collections::HashMap;
use std::sync::{Arc, Weak};

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell};

use super::constants::{BUCKET, MAX_RECORD_BYTES};
use super::custody_error::SnapshotCustodyError;
use super::record::CustodyRecord;
use super::stored::Stored;
use crate::DaytonaApi;

/// The durable record of every native snapshot this worker took, kept in a JetStream key-value bucket.
///
/// One record belongs to one source sandbox on one Daytona API. Updates are compare-and-set, and a per-source gate
/// keeps this process from racing itself.
pub struct SnapshotCustody {
    context: jetstream::Context,
    pub(super) origin: String,
    store: OnceCell<kv::Store>,
    gates: std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl SnapshotCustody {
    /// Custody for snapshots taken through `api`, stored in the bucket `context` reaches.
    pub fn new(context: jetstream::Context, api: &DaytonaApi) -> Self {
        Self {
            context,
            origin: hex::encode(Sha256::digest(api.config().endpoint().as_str().as_bytes())),
            store: OnceCell::new(),
            gates: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The gate every capture and cleanup of `source` holds while it runs.
    pub(crate) fn gate(&self, source: &str) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(source).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(source.to_owned(), Arc::downgrade(&gate));
        gate
    }

    fn key(&self, source: &str) -> String {
        format!("{}.{source}", self.origin)
    }

    async fn storage(&self) -> Result<&kv::Store, SnapshotCustodyError> {
        let store = self
            .store
            .get_or_try_init(|| {
                self.context.create_or_update_key_value(kv::Config {
                    bucket: BUCKET.into(),
                    history: 1,
                    ..Default::default()
                })
            })
            .await?;
        Ok(store)
    }

    fn encode(&self, record: &CustodyRecord) -> Result<Bytes, SnapshotCustodyError> {
        record.validate(&self.origin, &record.source)?;
        let bytes = serde_json::to_vec(record).map_err(SnapshotCustodyError::Encode)?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(SnapshotCustodyError::TooLarge);
        }
        Ok(Bytes::from(bytes))
    }

    pub(super) async fn load(&self, source: &str) -> Result<Option<Stored>, SnapshotCustodyError> {
        let Some(entry) = self.storage().await?.entry(self.key(source)).await? else {
            return Ok(None);
        };
        if entry.operation != kv::Operation::Put {
            return Err(SnapshotCustodyError::Removed);
        }
        if entry.value.len() > MAX_RECORD_BYTES {
            return Err(SnapshotCustodyError::TooLarge);
        }
        let record: CustodyRecord = serde_json::from_slice(&entry.value).map_err(SnapshotCustodyError::Decode)?;
        record.validate(&self.origin, source)?;
        Ok(Some(Stored {
            record,
            revision: entry.revision,
        }))
    }

    /// Stores a record that must not exist yet.
    pub(super) async fn create(&self, record: CustodyRecord) -> Result<Stored, SnapshotCustodyError> {
        let bytes = self.encode(&record)?;
        let revision = self.storage().await?.create(self.key(&record.source), bytes).await?;
        Ok(Stored { record, revision })
    }

    /// Replaces the record if it is still at the revision it was read at.
    pub(super) async fn save(&self, stored: Stored) -> Result<Stored, SnapshotCustodyError> {
        let bytes = self.encode(&stored.record)?;
        let revision = self
            .storage()
            .await?
            .update(self.key(&stored.record.source), bytes, stored.revision)
            .await?;
        Ok(Stored {
            record: stored.record,
            revision,
        })
    }

    /// Whether custody knows which organization owns `source`.
    pub(crate) async fn has_source_identity(&self, source: &str) -> Result<bool, SnapshotCustodyError> {
        Ok(self
            .load(source)
            .await?
            .is_some_and(|stored| stored.record.organization.as_deref().is_some_and(|id| !id.is_empty())))
    }
}
