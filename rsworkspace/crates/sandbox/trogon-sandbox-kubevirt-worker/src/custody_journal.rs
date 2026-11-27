use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trogon_sandbox_contract::{LeaseId, WorkerId};

use crate::constants::{
    CUSTODY_MAX_LEASE_LEN, CUSTODY_MAX_RECORDS, CUSTODY_MAX_RESOURCE_VERSION_LEN, CUSTODY_VERSION, LEASE_LABEL,
    TENANT_LABEL, WORKER_LABEL,
};
use crate::custody_files::{private_directory, private_file, read, require, save, sync_directory};
use crate::{BoundedName, CustodyBinding, CustodyError, CustodyPhase, CustodyRecord, ResourceReceipt};

/// The durable, single-owner record of what one worker created in the cluster.
///
/// It lives in a directory guarded by a lock file, so a second worker on the same path is refused, and every
/// lease's record survives a crash so the next start can find and settle what the last one left behind.
pub struct CustodyJournal {
    path: PathBuf,
    binding: CustodyBinding,
    lock: File,
}

impl Drop for CustodyJournal {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

impl CustodyJournal {
    /// Opens or creates the journal at `path` for `worker` in `namespace`, taking its lock.
    pub fn open(path: &Path, worker: WorkerId, namespace: &str) -> Result<Arc<Self>, CustodyError> {
        let binding = CustodyBinding {
            version: CUSTODY_VERSION,
            worker,
            namespace: BoundedName::new(namespace)?,
        };
        if path.exists() {
            require(std::fs::symlink_metadata(path)?.is_dir(), CustodyError::NotDirectory)?;
        }
        let owner = path.join("owner.json");
        if owner.exists() {
            require(read::<CustodyBinding>(&owner)? == binding, CustodyError::ForeignOwner)?;
        }
        std::fs::create_dir_all(path)?;
        private_directory(path)?;
        let lock_path = path.join("worker.lock");
        let lock = private_file(&lock_path)?.open(&lock_path)?;
        lock.try_lock().map_err(|_| CustodyError::AlreadyHeld)?;
        if owner.exists() {
            require(read::<CustodyBinding>(&owner)? == binding, CustodyError::ForeignOwner)?;
        } else {
            save(&owner, &binding)?;
        }
        let journal = Arc::new(Self {
            path: path.to_owned(),
            binding,
            lock,
        });
        journal.all()?;
        Ok(journal)
    }

    pub(crate) fn lease_file(&self, lease: &LeaseId) -> Result<PathBuf, CustodyError> {
        BoundedName::new(lease.as_str()).map_err(|_| CustodyError::InvalidLeaseIdentity)?;
        require(
            !lease.as_str().contains('.') && lease.as_str().len() <= CUSTODY_MAX_LEASE_LEN,
            CustodyError::InvalidLeaseIdentity,
        )?;
        Ok(self.path.join(format!("lease-{lease}.json")))
    }

    fn validate_receipt(&self, receipt: &ResourceReceipt) -> Result<(), CustodyError> {
        require(
            receipt.namespace == self.binding.namespace.as_str()
                && BoundedName::new(receipt.name.as_str()).is_ok()
                && BoundedName::new(receipt.uid.as_str()).is_ok(),
            CustodyError::InvalidResource,
        )
    }

    fn validate(&self, record: &CustodyRecord) -> Result<(), CustodyError> {
        require(
            record.version == CUSTODY_VERSION
                && record.worker == self.binding.worker
                && record.namespace == self.binding.namespace,
            CustodyError::ForeignRecord,
        )?;
        require(
            record.phase != CustodyPhase::Cleaned || record.resources.is_empty() && record.create.is_none(),
            CustodyError::SettledWithResources,
        )?;
        require(
            record.phase != CustodyPhase::Ready || record.ready_identity(),
            CustodyError::IncompleteReady,
        )?;
        self.lease_file(&record.lease_id)?;
        if let Some(vm) = &record.resources.vm {
            self.validate_receipt(vm)?;
        }
        if let Some(data_volume) = &record.resources.data_volume {
            self.validate_receipt(data_volume)?;
        }
        if let Some(claim) = &record.resources.pending_claim {
            self.validate_receipt(&claim.candidate.vm)?;
            self.validate_receipt(&claim.candidate.data_volume)?;
            let version = claim.candidate.resource_version.len();
            require(
                version > 0 && version <= CUSTODY_MAX_RESOURCE_VERSION_LEN,
                CustodyError::InvalidClaim,
            )?;
            require(claim.labels == self.labels(record), CustodyError::ClaimScope)?;
        }
        Ok(())
    }

    /// The labels that scope every resource of `record` to this worker, lease, and tenant.
    pub fn labels(&self, record: &CustodyRecord) -> BTreeMap<String, String> {
        BTreeMap::from([
            (WORKER_LABEL.to_owned(), self.binding.worker.to_string()),
            (LEASE_LABEL.to_owned(), record.lease_id.to_string()),
            (TENANT_LABEL.to_owned(), record.tenant.to_string()),
        ])
    }

    /// The record of `lease`, if one exists.
    pub fn get(&self, lease: &LeaseId) -> Result<Option<CustodyRecord>, CustodyError> {
        let path = self.lease_file(lease)?;
        if !path.try_exists()? {
            return Ok(None);
        }
        let record: CustodyRecord = read(&path)?;
        require(record.lease_id == *lease, CustodyError::LeaseMismatch)?;
        self.validate(&record)?;
        Ok(Some(record))
    }

    /// Every lease record, validated.
    pub fn all(&self) -> Result<Vec<CustodyRecord>, CustodyError> {
        let mut records = Vec::new();
        let mut count = 0;
        for entry in std::fs::read_dir(&self.path)? {
            count += 1;
            require(count <= CUSTODY_MAX_RECORDS * 2 + 4, CustodyError::DirectoryBudget)?;
            let name = entry?.file_name();
            let name = name.to_str().ok_or(CustodyError::InvalidFileName)?;
            if let Some(lease) = name.strip_prefix("lease-").and_then(|name| name.strip_suffix(".json")) {
                require(records.len() < CUSTODY_MAX_RECORDS, CustodyError::CapacityExhausted)?;
                records.push(self.get(&LeaseId::new(lease))?.ok_or(CustodyError::RecordUnavailable)?);
            } else {
                require(
                    matches!(name, "owner.json" | "owner.pending" | "worker.lock")
                        || name.starts_with("lease-") && name.ends_with(".pending"),
                    CustodyError::UnrecognizedContent,
                )?;
            }
        }
        Ok(records)
    }

    pub(crate) fn binding(&self) -> &CustodyBinding {
        &self.binding
    }

    pub(crate) fn store(&self, record: &CustodyRecord) -> Result<(), CustodyError> {
        self.validate(record)?;
        save(&self.lease_file(&record.lease_id)?, record)
    }

    pub(crate) fn remove(&self, lease: &LeaseId) -> Result<(), CustodyError> {
        std::fs::remove_file(self.lease_file(lease)?)?;
        sync_directory(&self.path)
    }
}
