use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{LeaseId, SnapshotPolicy, SnapshotPushTarget};
use trogon_sandbox_contract::{SnapshotReceipt, WorkerId};

use crate::snapshot::{LocalSnapshot, directory_info};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

pub use crate::lease_gates::LeaseGates;
pub use crate::settings::RetentionConfig;
pub use crate::vm_dir::RETAINED_DIRECTORY as DIRECTORY;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum RetainedState {
    Armed {
        reserved_bytes: u64,
    },
    Ready {
        bytes: u64,
        #[serde(default)]
        target: Option<SnapshotPushTarget>,
    },
    Exported {
        target: SnapshotPushTarget,
        receipt: SnapshotReceipt,
        pending_cleanup_bytes: Option<u64>,
    },
    Purged {
        pending_cleanup_bytes: Option<u64>,
    },
    Failed {
        message: String,
    },
}

impl RetainedState {
    fn bytes(&self) -> u64 {
        match self {
            Self::Armed { reserved_bytes } => *reserved_bytes,
            Self::Ready { bytes, .. } => *bytes,
            Self::Exported {
                pending_cleanup_bytes, ..
            }
            | Self::Purged { pending_cleanup_bytes } => pending_cleanup_bytes.unwrap_or(0),
            Self::Failed { .. } => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetainedRecord {
    pub lease_id: LeaseId,
    pub policy: SnapshotPolicy,
    pub expires_at: SystemTime,
    pub state: RetainedState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resource: Option<crate::attempt_resources::ResourceReceipt>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseExportError {
    #[error("lease has no retained snapshot")]
    Missing,
    #[error("lease snapshot was purged")]
    Purged,
    #[error("snapshot export target differs from the lease policy or prior receipt")]
    TargetMismatch,
    #[error("snapshot capture failed: {0}")]
    Failed(String),
}

pub struct SnapshotDepot {
    root: PathBuf,
    config: RetentionConfig,
    database: StdMutex<Connection>,
    database_identity: (u64, u64),
    _lock: File,
    spaces: StdMutex<HashMap<String, Arc<crate::space::DiskReservation>>>,
}

impl Drop for SnapshotDepot {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self._lock) {
            tracing::warn!(%error, "releasing snapshot depot lock failed");
        }
    }
}

impl SnapshotDepot {
    pub fn open(root: &Path, worker: &WorkerId, config: RetentionConfig) -> anyhow::Result<Self> {
        ensure!(
            config.max_records > 0
                && config.max_records <= 1024
                && config.max_bytes > 0
                && !config.export_ttl.is_zero(),
            "invalid released-snapshot retention limits"
        );
        match std::fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let stat = std::fs::symlink_metadata(root)?;
        ensure!(
            stat.is_dir()
                && !stat.file_type().is_symlink()
                && stat.uid() == unsafe { libc::geteuid() }
                && stat.permissions().mode() & 0o077 == 0,
            "released-snapshot directory must be private and owned by the worker"
        );
        let root = std::fs::canonicalize(root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".lock"))?;
        fs2::FileExt::try_lock_exclusive(&lock)
            .context("released-snapshot depot is already owned by another worker")?;
        let path = root.join("index.sqlite3");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let stat = file.metadata()?;
        ensure!(
            stat.is_file()
                && stat.uid() == unsafe { libc::geteuid() }
                && stat.permissions().mode() & 0o077 == 0
                && stat.len() <= 72 * 1024 * 1024,
            "released-snapshot database must be a private bounded regular file"
        );
        let mut database = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        database.busy_timeout(Duration::from_secs(1))?;
        database.pragma_update(None, "synchronous", "FULL")?;
        database.pragma_update(None, "max_page_count", (config.max_records * 18 + 32) as i64)?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS snapshots (lease TEXT PRIMARY KEY, expires INTEGER NOT NULL, body BLOB NOT NULL CHECK(length(body) <= 65536));
            CREATE INDEX IF NOT EXISTS snapshots_by_expiry ON snapshots(expires);")?;
        let version: Option<String> = tx
            .query_row("SELECT value FROM metadata WHERE key='version'", [], |row| row.get(0))
            .optional()?;
        match version {
            Some(version) => {
                ensure!(version == "1", "released-snapshot database version mismatch");
                let stored: String =
                    tx.query_row("SELECT value FROM metadata WHERE key='worker'", [], |row| row.get(0))?;
                ensure!(stored == worker.as_str(), "released-snapshot worker identity mismatch");
            }
            None => {
                ensure!(
                    tx.query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get::<_, i64>(0))? == 0,
                    "released-snapshot database metadata is missing"
                );
                ensure!(
                    !std::fs::read_dir(&root)?
                        .any(|entry| entry
                            .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("snapshot-"))),
                    "retained files exist without a depot database"
                );
                tx.execute(
                    "INSERT INTO metadata(key,value) VALUES ('version','1'),('worker',?1)",
                    [worker.as_str()],
                )?;
            }
        }
        tx.commit()?;
        File::open(&root)?.sync_all()?;
        File::open(root.parent().context("snapshot depot has no parent")?)?.sync_all()?;
        Ok(Self {
            root,
            config,
            database: StdMutex::new(database),
            database_identity: (stat.dev(), stat.ino()),
            _lock: lock,
            spaces: Default::default(),
        })
    }

    fn verify_database(&self) -> anyhow::Result<()> {
        let stat = std::fs::symlink_metadata(self.root.join("index.sqlite3"))?;
        ensure!(
            stat.is_file() && !stat.file_type().is_symlink() && (stat.dev(), stat.ino()) == self.database_identity,
            "released-snapshot database identity changed"
        );
        Ok(())
    }

    pub fn directory(&self, lease: &LeaseId) -> PathBuf {
        self.root.join(format!(
            "snapshot-{}",
            hex::encode(Sha256::digest(lease.as_str().as_bytes()))
        ))
    }

    pub fn get(&self, lease: &LeaseId) -> anyhow::Result<Option<RetainedRecord>> {
        self.verify_database()?;
        read_record(
            &self.database.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            lease,
        )
    }

    pub fn arm(
        &self,
        lease: &LeaseId,
        policy: &SnapshotPolicy,
        reserved: u64,
        lease_deadline: SystemTime,
    ) -> anyhow::Result<()> {
        self.arm_owned(lease, policy, reserved, lease_deadline, None)
    }

    pub(crate) fn arm_owned(
        &self,
        lease: &LeaseId,
        policy: &SnapshotPolicy,
        reserved: u64,
        lease_deadline: SystemTime,
        resource: Option<&crate::attempt_resources::ResourceReceipt>,
    ) -> anyhow::Result<()> {
        ensure!(
            policy.artifact_type == trogon_sandbox_contract::ArtifactType::FirecrackerSnapshotV1,
            "retained snapshot policy must use FirecrackerSnapshotV1"
        );
        self.verify_database()?;
        let mut database = self.database.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let records = read_records(&tx)?;
        let existing = records.iter().find(|entry| entry.lease_id == *lease);
        if let Some(entry) = existing {
            ensure!(
                entry.policy == *policy
                    && match (&entry.resource, resource) {
                        (Some(existing), Some(resource)) => existing.same_resource(resource),
                        (None, None) => true,
                        _ => false,
                    }
                    && matches!(entry.state, RetainedState::Armed { reserved_bytes } if reserved_bytes == reserved),
                "lease already has a different retained snapshot state"
            );
            return Ok(());
        }
        ensure!(
            records.len() < self.config.max_records,
            "released-snapshot record budget is exhausted"
        );
        let used = records.iter().try_fold(0u64, |used, entry| {
            used.checked_add(entry.state.bytes())
                .context("retained snapshot sizes overflow u64")
        })?;
        ensure!(
            used.checked_add(reserved)
                .is_some_and(|bytes| bytes <= self.config.max_bytes),
            "released-snapshot byte budget is exhausted"
        );
        let expires_at = lease_deadline
            .checked_add(self.config.export_ttl)
            .context("retained snapshot expiry overflows SystemTime")?;
        write_record(
            &tx,
            &RetainedRecord {
                lease_id: lease.clone(),
                policy: policy.clone(),
                expires_at,
                state: RetainedState::Armed {
                    reserved_bytes: reserved,
                },
                resource: resource.cloned(),
            },
        )?;
        tx.commit()?;
        Ok(())
    }

    pub async fn retain(&self, lease: &LeaseId, snapshot: LocalSnapshot) -> anyhow::Result<()> {
        let record = self.get(lease)?.context("snapshot retention was not reserved")?;
        let RetainedState::Armed { reserved_bytes } = record.state else {
            anyhow::bail!("snapshot retention is not armed");
        };
        let info = directory_info(snapshot.path()).await?;
        ensure!(
            info.bytes <= reserved_bytes,
            "snapshot exceeds its reserved storage budget"
        );
        let destination = self.directory(lease);
        match tokio::fs::symlink_metadata(&destination).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => anyhow::bail!("retained snapshot files already exist"),
        }
        self.spaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(lease.as_str().to_owned(), snapshot.space());
        let bytes = match snapshot.seal_to(&destination).await {
            Ok(bytes) => bytes,
            Err(error) => {
                if !destination.exists() {
                    self.spaces
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(lease.as_str());
                }
                return Err(error);
            }
        };
        self.update(lease, |entry| {
            ensure!(
                matches!(entry.state, RetainedState::Armed { .. }),
                "snapshot retention changed during capture"
            );
            entry.state = RetainedState::Ready { bytes, target: None };
            entry.expires_at = SystemTime::now()
                .checked_add(self.config.export_ttl)
                .context("snapshot expiry overflows")?;
            Ok(())
        })
    }

    pub async fn recover(&self) -> anyhow::Result<()> {
        self.verify_database()?;
        let records = read_records(&self.database.lock().unwrap_or_else(std::sync::PoisonError::into_inner))?;
        for entry in records {
            self.recover_lease(&entry.lease_id).await?;
        }
        Ok(())
    }

    pub async fn recover_lease(&self, lease: &LeaseId) -> anyhow::Result<()> {
        if let Some(entry) = self.get(lease)? {
            let directory = self.directory(&entry.lease_id);
            if let Ok(stat) = tokio::fs::symlink_metadata(&directory).await {
                ensure!(
                    stat.is_dir() && !stat.file_type().is_symlink(),
                    "retained snapshot directory is not a real directory"
                );
            }
            match entry.state {
                RetainedState::Armed { reserved_bytes } if directory.exists() => {
                    let info = directory_info(&directory).await?;
                    ensure!(
                        info.bytes <= reserved_bytes,
                        "interrupted snapshot exceeds its reservation"
                    );
                    self.update(&entry.lease_id, |record| {
                        record.state = RetainedState::Ready {
                            bytes: info.bytes,
                            target: None,
                        };
                        record.expires_at = SystemTime::now()
                            .checked_add(self.config.export_ttl)
                            .context("snapshot expiry overflows")?;
                        Ok(())
                    })?;
                }
                RetainedState::Ready { bytes, .. } => {
                    ensure!(
                        directory_info(&directory).await?.bytes == bytes,
                        "retained snapshot byte count changed"
                    );
                }
                RetainedState::Exported {
                    pending_cleanup_bytes: Some(_),
                    ..
                }
                | RetainedState::Purged {
                    pending_cleanup_bytes: Some(_),
                } => {
                    self.cleanup_files(&entry.lease_id).await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn check_target(&self, entry: &RetainedRecord, target: &SnapshotPushTarget) -> anyhow::Result<()> {
        if entry.policy.artifact_type != target.artifact_type || entry.policy.target_repository != target.repository {
            return Err(ReleaseExportError::TargetMismatch.into());
        }
        match &entry.state {
            RetainedState::Ready {
                target: Some(prior), ..
            } if prior != target => Err(ReleaseExportError::TargetMismatch.into()),
            RetainedState::Exported { target: prior, .. } if prior != target => {
                Err(ReleaseExportError::TargetMismatch.into())
            }
            RetainedState::Purged { .. } => Err(ReleaseExportError::Purged.into()),
            RetainedState::Failed { message } => Err(ReleaseExportError::Failed(message.clone()).into()),
            _ => Ok(()),
        }
    }

    pub fn receipt(&self, lease: &LeaseId, target: &SnapshotPushTarget) -> anyhow::Result<Option<SnapshotReceipt>> {
        let Some(entry) = self.get(lease)? else {
            return Err(ReleaseExportError::Missing.into());
        };
        self.check_target(&entry, target)?;
        Ok(match entry.state {
            RetainedState::Exported { receipt, .. } => Some(receipt),
            _ => None,
        })
    }

    pub fn record_receipt(
        &self,
        lease: &LeaseId,
        target: &SnapshotPushTarget,
        receipt: &SnapshotReceipt,
    ) -> anyhow::Result<()> {
        ensure!(
            receipt.artifact_type == target.artifact_type && receipt.bytes > 0,
            "snapshot export receipt is invalid"
        );
        self.update(lease, |entry| {
            self.check_target(entry, target)?;
            let RetainedState::Ready { bytes, target: Some(_) } = entry.state else {
                anyhow::bail!("snapshot is not ready for export");
            };
            entry.state = RetainedState::Exported {
                target: target.clone(),
                receipt: receipt.clone(),
                pending_cleanup_bytes: Some(bytes),
            };
            Ok(())
        })
    }

    pub fn claim_target(&self, lease: &LeaseId, target: &SnapshotPushTarget) -> anyhow::Result<()> {
        self.update(lease, |entry| {
            self.check_target(entry, target)?;
            let RetainedState::Ready { target: prior, .. } = &mut entry.state else {
                anyhow::bail!("snapshot is not ready for export");
            };
            *prior = Some(target.clone());
            Ok(())
        })
    }

    pub async fn cleanup_files(&self, lease: &LeaseId) -> anyhow::Result<()> {
        let Some(record) = self.get(lease)? else {
            return Ok(());
        };
        ensure!(
            matches!(
                record.state,
                RetainedState::Exported { .. } | RetainedState::Purged { .. }
            ),
            "snapshot files still needed for export"
        );
        if let Ok(stat) = tokio::fs::symlink_metadata(self.directory(lease)).await {
            ensure!(
                stat.is_dir() && !stat.file_type().is_symlink(),
                "retained snapshot path is not a real directory"
            );
        }
        match tokio::fs::remove_dir_all(self.directory(lease)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::fs::File::open(&self.root)?.sync_all()?;
        self.spaces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(lease.as_str());
        self.update(lease, |entry| {
            match &mut entry.state {
                RetainedState::Exported {
                    pending_cleanup_bytes, ..
                }
                | RetainedState::Purged { pending_cleanup_bytes } => *pending_cleanup_bytes = None,
                _ => anyhow::bail!("snapshot cleanup state changed"),
            }
            Ok(())
        })
    }

    pub async fn mark_purged(&self, lease: &LeaseId) -> anyhow::Result<()> {
        if self.get(lease)?.is_none() {
            return Ok(());
        }
        self.update(lease, |entry| {
            let bytes = match entry.state {
                RetainedState::Failed { .. } => None,
                _ => Some(entry.state.bytes()),
            };
            entry.state = RetainedState::Purged {
                pending_cleanup_bytes: bytes,
            };
            Ok(())
        })?;
        self.cleanup_files(lease).await
    }

    pub fn fail_if_armed(&self, lease: &LeaseId, message: &str) -> anyhow::Result<()> {
        if !self
            .get(lease)?
            .is_some_and(|entry| matches!(entry.state, RetainedState::Armed { .. }))
        {
            return Ok(());
        }
        self.update(lease, |entry| {
            entry.state = RetainedState::Failed {
                message: message.into(),
            };
            Ok(())
        })
    }

    pub fn delete_purged(&self, lease: &LeaseId) -> anyhow::Result<()> {
        self.verify_database()?;
        let database = self.database.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = read_record(&database, lease)? {
            ensure!(
                matches!(
                    record.state,
                    RetainedState::Purged {
                        pending_cleanup_bytes: None
                    }
                ),
                "snapshot purge is incomplete"
            );
        }
        database.execute("DELETE FROM snapshots WHERE lease=?1", [&lease.as_str()])?;
        Ok(())
    }

    pub fn expired(&self, now: SystemTime) -> anyhow::Result<Vec<LeaseId>> {
        self.verify_database()?;
        Ok(
            read_records(&self.database.lock().unwrap_or_else(std::sync::PoisonError::into_inner))?
                .into_iter()
                .filter(|entry| entry.expires_at <= now)
                .map(|entry| entry.lease_id)
                .collect(),
        )
    }

    fn update(
        &self,
        lease: &LeaseId,
        change: impl FnOnce(&mut RetainedRecord) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.verify_database()?;
        let mut database = self.database.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut entry = read_record(&tx, lease)?.context("retained snapshot record is missing")?;
        change(&mut entry)?;
        write_record(&tx, &entry)?;
        tx.commit()?;
        Ok(())
    }
}

fn read_record(database: &Connection, lease: &LeaseId) -> anyhow::Result<Option<RetainedRecord>> {
    let body: Option<Vec<u8>> = database
        .query_row("SELECT body FROM snapshots WHERE lease=?1", [&lease.as_str()], |row| {
            row.get(0)
        })
        .optional()?;
    body.map(|body| {
        ensure!(body.len() <= 65536, "retained snapshot record exceeds storage budget");
        let entry: RetainedRecord = serde_json::from_slice(&body)?;
        ensure!(entry.lease_id == *lease, "retained snapshot lease identity mismatch");
        Ok(entry)
    })
    .transpose()
}

fn read_records(database: &Connection) -> anyhow::Result<Vec<RetainedRecord>> {
    let mut statement = database.prepare("SELECT lease FROM snapshots")?;
    let mut entries = Vec::new();
    for lease in statement.query_map([], |row| row.get::<_, String>(0))? {
        ensure!(
            entries.len() < 1024,
            "retained snapshot record count exceeds hard limit"
        );
        entries.push(read_record(database, &LeaseId::new(lease?))?.context("snapshot record disappeared")?);
    }
    Ok(entries)
}

fn write_record(database: &Connection, entry: &RetainedRecord) -> anyhow::Result<()> {
    let body = serde_json::to_vec(entry)?;
    ensure!(body.len() <= 65536, "retained snapshot record exceeds storage budget");
    let expiry: i64 = entry.expires_at.duration_since(UNIX_EPOCH)?.as_secs().try_into()?;
    database.execute("INSERT INTO snapshots(lease,expires,body) VALUES (?1,?2,?3) ON CONFLICT(lease) DO UPDATE SET expires=excluded.expires,body=excluded.body", params![entry.lease_id.as_str(), expiry, body])?;
    Ok(())
}

#[cfg(test)]
mod tests;
