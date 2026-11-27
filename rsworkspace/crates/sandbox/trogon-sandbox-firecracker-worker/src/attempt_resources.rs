use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use trogon_sandbox_contract::LeaseAttemptIdentity;

use crate::vm_dir::VmDir;

mod constants;
use constants::{MARKER, MAX_RECORD_BYTES, MAX_RECORDS, PREFIX};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ResourceReceipt {
    pub identity: LeaseAttemptIdentity,
    nonce: String,
    pub effects_started: bool,
    pub settled: bool,
}

impl ResourceReceipt {
    pub fn same_resource(&self, other: &Self) -> bool {
        self.identity == other.identity && self.nonce == other.nonce
    }
}

pub(crate) struct AttemptResources {
    root: PathBuf,
    writes: Mutex<()>,
}

impl AttemptResources {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            writes: Mutex::new(()),
        }
    }

    fn path(&self, identity: &LeaseAttemptIdentity) -> PathBuf {
        self.root.join(format!("{PREFIX}{}.json", identity.target.attempt_id))
    }

    pub async fn read(&self, identity: &LeaseAttemptIdentity) -> anyhow::Result<ResourceReceipt> {
        let receipt = read_record(&self.path(identity)).await?;
        ensure!(
            receipt.identity == *identity,
            "VM resource journal belongs to another assignment"
        );
        Ok(receipt)
    }

    pub async fn read_optional(&self, identity: &LeaseAttemptIdentity) -> anyhow::Result<Option<ResourceReceipt>> {
        match self.read(identity).await {
            Ok(receipt) => Ok(Some(receipt)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn begin(&self, identity: &LeaseAttemptIdentity) -> anyhow::Result<ResourceReceipt> {
        let _write = self.writes.lock().await;
        let path = self.path(identity);
        ensure!(
            !tokio::fs::try_exists(&path).await?,
            "VM resource assignment already has a journal"
        );
        let mut entries = tokio::fs::read_dir(&self.root).await?;
        let mut count = 0;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_name().to_string_lossy().starts_with(PREFIX) {
                count += 1;
                ensure!(count < MAX_RECORDS, "VM resource journal capacity exhausted");
            }
        }
        let mut entropy = [0u8; 24];
        getrandom::fill(&mut entropy).map_err(|error| anyhow::anyhow!("creating VM resource identity: {error}"))?;
        let receipt = ResourceReceipt {
            identity: identity.clone(),
            nonce: hex::encode(entropy),
            effects_started: false,
            settled: false,
        };
        save_record(&path, &receipt).await?;
        Ok(receipt)
    }

    pub async fn attach(&self, dir: &VmDir, receipt: &ResourceReceipt) -> anyhow::Result<()> {
        let current = self.read(&receipt.identity).await?;
        ensure!(
            current.same_resource(receipt) && current.effects_started && !current.settled,
            "VM effects have no durable allocation intent"
        );
        let path = dir.as_path().join(MARKER);
        ensure!(
            !tokio::fs::try_exists(&path).await?,
            "VM directory already has an allocation owner"
        );
        save_record(&path, receipt).await
    }

    pub async fn start_effects(&self, receipt: &ResourceReceipt) -> anyhow::Result<ResourceReceipt> {
        let _write = self.writes.lock().await;
        let mut current = self.read(&receipt.identity).await?;
        ensure!(
            current.same_resource(receipt) && !current.settled,
            "VM allocation identity is unconfirmed"
        );
        current.effects_started = true;
        save_record(&self.path(&receipt.identity), &current).await?;
        Ok(current)
    }

    pub async fn directory_receipt(&self, dir: &VmDir) -> anyhow::Result<Option<ResourceReceipt>> {
        let path = dir.as_path().join(MARKER);
        match read_record(&path).await {
            Ok(receipt) => Ok(Some(receipt)),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub async fn confirm_directory(&self, dir: &VmDir, receipt: &ResourceReceipt) -> anyhow::Result<()> {
        let actual = self
            .directory_receipt(dir)
            .await?
            .context("VM directory has no exact allocation owner")?;
        ensure!(
            actual.same_resource(receipt),
            "VM directory belongs to another resource incarnation"
        );
        Ok(())
    }

    pub async fn complete(&self, receipt: &ResourceReceipt) -> anyhow::Result<()> {
        let _write = self.writes.lock().await;
        let mut current = self.read(&receipt.identity).await?;
        ensure!(
            current.same_resource(receipt),
            "VM resource completion identity differs"
        );
        current.settled = true;
        save_record(&self.path(&receipt.identity), &current).await
    }

    pub async fn forget(&self, identity: &LeaseAttemptIdentity) -> anyhow::Result<()> {
        let _write = self.writes.lock().await;
        let receipt = match self.read(identity).await {
            Ok(receipt) => receipt,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        ensure!(receipt.settled, "VM resource completion is unconfirmed");
        tokio::fs::remove_file(self.path(identity)).await?;
        tokio::fs::File::open(&self.root).await?.sync_all().await?;
        Ok(())
    }
}

async fn read_record(path: &Path) -> anyhow::Result<ResourceReceipt> {
    ensure!(
        tokio::fs::metadata(path).await?.len() <= MAX_RECORD_BYTES,
        "VM resource journal exceeds its budget"
    );
    let receipt: ResourceReceipt =
        serde_json::from_slice(&tokio::fs::read(path).await?).context("invalid VM resource journal")?;
    ensure!(
        receipt.identity.provider == trogon_sandbox_contract::ProviderKind::Firecracker
            && receipt.nonce.len() == 48
            && receipt.nonce.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid VM resource identity"
    );
    Ok(receipt)
}

async fn save_record(path: &Path, receipt: &ResourceReceipt) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(receipt)?;
    ensure!(
        bytes.len() as u64 <= MAX_RECORD_BYTES,
        "VM resource journal exceeds its budget"
    );
    let pending = path.with_extension("pending");
    let mut file = tokio::fs::File::create(&pending).await?;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(pending, path).await?;
    tokio::fs::File::open(path.parent().context("VM resource journal has no parent")?)
        .await?
        .sync_all()
        .await?;
    Ok(())
}
