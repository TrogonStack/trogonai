use std::sync::OnceLock;
use std::time::Duration;

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use tokio::sync::OnceCell;
use trogon_sandbox_contract::CancelOwnedLeaseRequest;
use trogon_sandbox_protocol::tenant_scoped_kv_key;

use super::constants::{BUCKET, MAX_BEGIN_ROUNDS, MAX_RECORD_BYTES};
use super::lease_cleanup_error::LeaseCleanupError;
use super::phase::Phase;
use super::receipt::Receipt;
use super::request_check::validate_request;
use super::stored_cleanup_receipt::StoredCleanupReceipt;
use crate::bounded_retry::bounded_retry;
use crate::custody_bucket::{ensure_durable, open_custody_bucket};
use crate::kv_outcome::created;
use crate::snapshot_custody::{after_ms, now_ms};

/// The KV store of owned-cleanup receipts, bound to JetStream on first use.
#[derive(Default)]
pub(crate) struct LeaseCleanupReceipts {
    context: OnceLock<jetstream::Context>,
    store: OnceCell<kv::Store>,
}

pub(super) fn key(request: &CancelOwnedLeaseRequest) -> Result<String, LeaseCleanupError> {
    Ok(tenant_scoped_kv_key(
        &request.tenant,
        &serde_json::to_string(&(request.lease_id.as_str(), request.sandbox_id.as_str()))?,
    ))
}

fn bytes(receipt: &Receipt) -> Result<Bytes, LeaseCleanupError> {
    let bytes = serde_json::to_vec(receipt)?;
    within_limit(bytes.len())?;
    Ok(Bytes::from(bytes))
}

/// Refuses a receipt record longer than the limit.
pub(super) fn within_limit(length: usize) -> Result<(), LeaseCleanupError> {
    if length > MAX_RECORD_BYTES {
        return Err(LeaseCleanupError::TooLarge);
    }
    Ok(())
}

impl LeaseCleanupReceipts {
    pub(crate) fn is_configured(&self) -> bool {
        self.context.get().is_some()
    }

    pub(crate) fn configure(&self, context: jetstream::Context) -> Result<(), LeaseCleanupError> {
        self.context
            .set(context)
            .map_err(|_| LeaseCleanupError::AlreadyConfigured)
    }

    async fn storage(&self) -> Result<&kv::Store, LeaseCleanupError> {
        let context = self.context.get().ok_or(LeaseCleanupError::NotConfigured)?;
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

    pub(crate) async fn load(
        &self,
        request: &CancelOwnedLeaseRequest,
    ) -> Result<Option<StoredCleanupReceipt>, LeaseCleanupError> {
        validate_request(request)?;
        let Some(entry) = self.storage().await?.entry(key(request)?).await? else {
            return Ok(None);
        };
        if entry.operation != kv::Operation::Put {
            return Ok(None);
        }
        within_limit(entry.value.len())?;
        let receipt: Receipt = serde_json::from_slice(&entry.value).map_err(LeaseCleanupError::Decode)?;
        receipt.validate(request)?;
        Ok(Some(StoredCleanupReceipt {
            receipt,
            revision: entry.revision,
        }))
    }

    /// The receipt for `request`, written pending when none exists yet.
    pub(crate) async fn begin(
        &self,
        request: &CancelOwnedLeaseRequest,
    ) -> Result<StoredCleanupReceipt, LeaseCleanupError> {
        bounded_retry(MAX_BEGIN_ROUNDS, LeaseCleanupError::Contended, || async {
            if let Some(stored) = self.load(request).await? {
                return Ok(Some(stored));
            }
            let receipt = Receipt::pending(request)?;
            let revision = created(self.storage().await?.create(key(request)?, bytes(&receipt)?).await)?;
            Ok(revision.map(|revision| StoredCleanupReceipt { receipt, revision }))
        })
        .await
    }

    async fn save(
        &self,
        request: &CancelOwnedLeaseRequest,
        stored: StoredCleanupReceipt,
    ) -> Result<StoredCleanupReceipt, LeaseCleanupError> {
        stored.receipt.validate(request)?;
        let revision = self
            .storage()
            .await?
            .update(key(request)?, bytes(&stored.receipt)?, stored.revision)
            .await?;
        Ok(StoredCleanupReceipt {
            receipt: stored.receipt,
            revision,
        })
    }

    /// Takes the cleanup for `duration`, so only one worker deletes the sandbox at a time.
    pub(crate) async fn claim(
        &self,
        request: &CancelOwnedLeaseRequest,
        mut stored: StoredCleanupReceipt,
        duration: Duration,
    ) -> Result<StoredCleanupReceipt, LeaseCleanupError> {
        if stored.absent() || stored.receipt.claim_until_unix_ms > now_ms()? {
            return Err(LeaseCleanupError::InProgress);
        }
        stored.receipt.claim_until_unix_ms = after_ms(duration)?;
        self.save(request, stored).await
    }

    pub(crate) async fn release(
        &self,
        request: &CancelOwnedLeaseRequest,
        mut stored: StoredCleanupReceipt,
    ) -> Result<(), LeaseCleanupError> {
        stored.receipt.claim_until_unix_ms = 0;
        self.save(request, stored).await?;
        Ok(())
    }

    pub(crate) async fn confirm_absent(
        &self,
        request: &CancelOwnedLeaseRequest,
        mut stored: StoredCleanupReceipt,
    ) -> Result<(), LeaseCleanupError> {
        stored.receipt.phase = Phase::Absent;
        stored.receipt.claim_until_unix_ms = 0;
        self.save(request, stored).await?;
        Ok(())
    }
}
