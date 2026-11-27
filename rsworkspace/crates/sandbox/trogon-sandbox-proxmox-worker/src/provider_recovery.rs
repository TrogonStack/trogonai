use trogon_sandbox_contract::LeaseId;

use crate::{LeaseRecord, ProxmoxProvider, ProxmoxProviderError};

impl ProxmoxProvider {
    /// Destroys every VM a previous run of this worker journaled. Returns how many were destroyed.
    pub async fn recover(&self) -> Result<usize, ProxmoxProviderError> {
        tokio::fs::create_dir_all(&self.config.state_dir).await?;
        let mut entries = tokio::fs::read_dir(&self.config.state_dir).await?;
        let mut recovered = 0;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let record: LeaseRecord = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
            let record = self.latest_record(record).await?;
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or(ProxmoxProviderError::Backend("invalid journal name"))?;
            let lease = LeaseId::new(stem);
            self.journal_path(&lease)?;
            self.destroy_owned(&lease, &record).await?;
            self.remove_journal(&lease).await?;
            recovered += 1;
        }
        Ok(recovered)
    }

    /// Destroys every VM this worker holds. Every lease is attempted; the last failure is returned.
    pub async fn shutdown(&self) -> Result<(), ProxmoxProviderError> {
        let leases: Vec<LeaseId> = self.active.lock().await.keys().cloned().collect();
        let mut outcome = Ok(());
        for lease in leases {
            if let Err(error) = self.release(&lease).await {
                outcome = Err(error);
            }
        }
        outcome
    }
}
