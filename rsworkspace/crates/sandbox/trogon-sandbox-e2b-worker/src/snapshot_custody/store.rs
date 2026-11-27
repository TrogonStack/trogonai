use std::sync::OnceLock;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use tokio::sync::{Mutex, OnceCell};
use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::constants::{BUCKET, MAX_RECORD_BYTES, SCAN_BATCH};
use super::custody_error::SnapshotCustodyError;
use super::record::CustodyRecord;
use super::stored::StoredCustody;
use crate::custody_bucket::{ensure_durable, open_custody_bucket};
use crate::snapshots::validate_reference_identity;

/// The KV store of custody records, bound to JetStream on first use.
#[derive(Default)]
pub(crate) struct SnapshotCustody {
    context: OnceLock<jetstream::Context>,
    store: OnceCell<kv::Store>,
    cursor: Mutex<u64>,
}

impl SnapshotCustody {
    pub(crate) fn configure(&self, context: jetstream::Context) -> Result<(), SnapshotCustodyError> {
        self.context
            .set(context)
            .map_err(|_| SnapshotCustodyError::AlreadyConfigured)
    }

    async fn storage(&self) -> Result<&kv::Store, SnapshotCustodyError> {
        let context = self.context.get().ok_or(SnapshotCustodyError::NotConfigured)?;
        let store = self
            .store
            .get_or_try_init(|| {
                open_custody_bucket(
                    context,
                    kv::Config {
                        bucket: BUCKET.into(),
                        history: 1,
                        ..Default::default()
                    },
                )
            })
            .await?;
        ensure_durable(store).await?;
        Ok(store)
    }

    /// A validated record is far below the byte limit, so only reads check it.
    fn encode(record: &CustodyRecord) -> Result<Bytes, SnapshotCustodyError> {
        record.validate()?;
        let bytes = serde_json::to_vec(record).map_err(SnapshotCustodyError::Encode)?;
        Ok(Bytes::from(bytes))
    }

    fn decode(payload: &[u8]) -> Result<CustodyRecord, SnapshotCustodyError> {
        if payload.len() > MAX_RECORD_BYTES {
            return Err(SnapshotCustodyError::TooLarge);
        }
        let record: CustodyRecord = serde_json::from_slice(payload).map_err(SnapshotCustodyError::Decode)?;
        record.validate()?;
        Ok(record)
    }

    pub(crate) async fn create(&self, record: CustodyRecord) -> Result<StoredCustody, SnapshotCustodyError> {
        let bytes = Self::encode(&record)?;
        let revision = self.storage().await?.create(record.key(), bytes).await?;
        Ok(StoredCustody { record, revision })
    }

    pub(crate) async fn load(
        &self,
        tenant: &TenantId,
        reference: &NativeSnapshotRef,
    ) -> Result<Option<StoredCustody>, SnapshotCustodyError> {
        validate_reference_identity(reference)?;
        let key = tenant_scoped_kv_key(tenant, reference.id.as_str());
        let Some(entry) = self.storage().await?.entry(&key).await? else {
            return Ok(None);
        };
        if entry.operation != kv::Operation::Put {
            return Ok(None);
        }
        let record = Self::decode(&entry.value)?;
        if record.key() != key || &record.tenant != tenant || record.reference.provider != reference.provider {
            return Err(SnapshotCustodyError::IdentityMismatch);
        }
        Ok(Some(StoredCustody {
            record,
            revision: entry.revision,
        }))
    }

    /// Replaces the record if it is still at `revision`.
    pub(crate) async fn save(
        &self,
        record: CustodyRecord,
        revision: u64,
    ) -> Result<StoredCustody, SnapshotCustodyError> {
        let bytes = Self::encode(&record)?;
        let revision = self.storage().await?.update(record.key(), bytes, revision).await?;
        Ok(StoredCustody { record, revision })
    }

    /// Examines the next batch of stream sequences and returns the live records in it, wrapping around at the end.
    pub(crate) async fn scan(&self) -> Result<Vec<StoredCustody>, SnapshotCustodyError> {
        let store = self.storage().await?;
        let mut stream = store.stream.clone();
        let last = stream.info().await?.state.last_sequence;
        let mut cursor = self.cursor.lock().await;
        if *cursor >= last {
            *cursor = 0;
        }
        let end = cursor.saturating_add(SCAN_BATCH).min(last);
        let mut found = Vec::new();
        while *cursor < end {
            *cursor += 1;
            let Some(message) = live_message(stream.get_raw_message(*cursor).await)? else {
                continue;
            };
            if message.payload.is_empty() {
                continue;
            }
            let record = Self::decode(&message.payload)?;
            if message.subject.as_str() != format!("{}{}", store.prefix, record.key()) {
                return Err(SnapshotCustodyError::SubjectMismatch);
            }
            if let Some(current) = self.load(&record.tenant, &record.reference).await?
                && current.revision == message.sequence
            {
                found.push(current);
            }
        }
        Ok(found)
    }
}

/// The message at a stream sequence, or `None` when that sequence holds nothing because it was deleted or expired.
pub(super) fn live_message(
    read: Result<jetstream::message::StreamMessage, jetstream::stream::RawMessageError>,
) -> Result<Option<jetstream::message::StreamMessage>, SnapshotCustodyError> {
    match read {
        Ok(message) => Ok(Some(message)),
        Err(error) if error.kind() == jetstream::stream::RawMessageErrorKind::NoMessageFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
