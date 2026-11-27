use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use trogon_sandbox_contract::{LeaseAttemptIdentity, LeaseId};

use crate::check::ensure;
use crate::constants::{ATTEMPTS_DIR, MAX_ATTEMPT_JOURNALS, MAX_JOURNAL_BYTES, MAX_LEASE_ID_LEN};
use crate::{LeaseRecord, ProxmoxProvider, ProxmoxProviderError};

async fn sync_dir(path: &Path) -> Result<(), ProxmoxProviderError> {
    tokio::fs::File::open(path).await?.sync_all().await?;
    Ok(())
}

/// Reads a journal, or `None` when there is none.
pub(crate) async fn read_journal(path: &Path) -> Result<Option<LeaseRecord>, ProxmoxProviderError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl ProxmoxProvider {
    pub(crate) fn journal_path(&self, lease: &LeaseId) -> Result<PathBuf, ProxmoxProviderError> {
        let id = lease.as_str();
        let valid = !id.is_empty()
            && id.len() <= MAX_LEASE_ID_LEN
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        ensure(valid, ProxmoxProviderError::Rejected("invalid lease id"))?;
        Ok(self.config.state_dir.join(format!("{id}.json")))
    }

    pub(crate) fn attempt_path(&self, identity: &LeaseAttemptIdentity) -> PathBuf {
        self.config
            .state_dir
            .join(ATTEMPTS_DIR)
            .join(format!("{}.json", identity.target.attempt_id.as_str()))
    }

    /// The journal of the exact attempt `record` belongs to, which must describe the same VM.
    pub(crate) async fn latest_record(&self, record: LeaseRecord) -> Result<LeaseRecord, ProxmoxProviderError> {
        let Some(identity) = &record.assignment else {
            return Ok(record);
        };
        let latest = read_journal(&self.attempt_path(identity))
            .await?
            .ok_or(ProxmoxProviderError::Backend(
                "exact VM allocation journal is unavailable",
            ))?;
        ensure(
            latest.same_allocation(&record),
            ProxmoxProviderError::Backend("VM allocation journal identity differs"),
        )?;
        Ok(latest)
    }

    /// Writes the lease's journal, and the attempt's when the lease belongs to a dispatch attempt.
    pub(crate) async fn save(&self, lease: &LeaseId, record: &LeaseRecord) -> Result<(), ProxmoxProviderError> {
        tokio::fs::create_dir_all(&self.config.state_dir).await?;
        let path = self.journal_path(lease)?;
        if let Some(identity) = &record.assignment {
            ensure(
                identity.lease_id == *lease,
                ProxmoxProviderError::Backend("VM assignment lease differs"),
            )?;
            let attempts = self.config.state_dir.join(ATTEMPTS_DIR);
            tokio::fs::create_dir_all(&attempts).await?;
            let attempt = self.attempt_path(identity);
            if !tokio::fs::try_exists(&attempt).await? {
                let mut entries = tokio::fs::read_dir(&attempts).await?;
                let mut count = 0;
                while entries.next_entry().await?.is_some() {
                    count += 1;
                    ensure(
                        count < MAX_ATTEMPT_JOURNALS,
                        ProxmoxProviderError::Backend("VM allocation journal capacity exhausted"),
                    )?;
                }
            }
            self.save_record(&attempt, record).await?;
            sync_dir(&self.config.state_dir).await?;
        }
        self.save_record(&path, record).await
    }

    async fn save_record(&self, path: &Path, record: &LeaseRecord) -> Result<(), ProxmoxProviderError> {
        let bytes = serde_json::to_vec(record)?;
        ensure(
            bytes.len() <= MAX_JOURNAL_BYTES,
            ProxmoxProviderError::Backend("VM allocation journal exceeds its budget"),
        )?;
        let pending = path.with_extension("pending");
        let mut file = tokio::fs::File::create(&pending).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&pending, path).await?;
        let parent = path
            .parent()
            .ok_or(ProxmoxProviderError::Backend("VM journal parent is missing"))?;
        sync_dir(parent).await
    }

    pub(crate) async fn remove_journal(&self, lease: &LeaseId) -> Result<(), ProxmoxProviderError> {
        match tokio::fs::remove_file(self.journal_path(lease)?).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        sync_dir(&self.config.state_dir).await
    }

    /// Removes an attempt's journal once its VM is proven destroyed.
    pub(crate) async fn remove_attempt_journal(&self, path: &Path) -> Result<(), ProxmoxProviderError> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {
                let parent = path
                    .parent()
                    .ok_or(ProxmoxProviderError::Backend("VM journal parent is missing"))?;
                sync_dir(parent).await
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
