use async_trait::async_trait;
use trogon_sandbox_contract::DriveRecord;

use super::{DriveDataPurger, DrivePurgeError};

/// Purger that removes nothing.
pub struct NoopDriveDataPurger;

#[async_trait]
impl DriveDataPurger for NoopDriveDataPurger {
    async fn purge(&self, _record: &DriveRecord) -> Result<(), DrivePurgeError> {
        Ok(())
    }
}
