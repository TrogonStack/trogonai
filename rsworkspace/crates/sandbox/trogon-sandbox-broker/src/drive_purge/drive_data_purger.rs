use async_trait::async_trait;
use trogon_sandbox_contract::DriveRecord;

use super::DrivePurgeError;

/// Removes a drive's stored data when the drive is deleted.
#[async_trait]
pub trait DriveDataPurger: Send + Sync {
    async fn purge(&self, record: &DriveRecord) -> Result<(), DrivePurgeError>;
}
