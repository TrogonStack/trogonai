use std::time::Duration;

use trogon_sandbox_contract::{
    CreateDriveRequest, CreateDriveResponse, DeleteDriveRequest, DeleteDriveResponse, DriveId, DriveRecord,
    DriveStorage, ListDrivesRequest, ListDrivesResponse, TenantId,
};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient};

const DRIVE_TIMEOUT: Duration = Duration::from_secs(2);

impl SandboxClient {
    /// Creates a drive backed by the S3 `bucket`.
    pub async fn create_drive(
        &self,
        tenant: TenantId,
        drive_id: DriveId,
        size_gib: Option<u64>,
        bucket: String,
    ) -> Result<DriveRecord, ClientError> {
        self.create_drive_with_storage(tenant, drive_id, size_gib, DriveStorage::S3 { bucket })
            .await
    }

    /// Creates a drive on `storage`. Creating a drive that already exists returns its record.
    pub async fn create_drive_with_storage(
        &self,
        tenant: TenantId,
        drive_id: DriveId,
        size_gib: Option<u64>,
        storage: DriveStorage,
    ) -> Result<DriveRecord, ClientError> {
        let request = CreateDriveRequest {
            tenant,
            drive_id,
            size_gib,
            storage,
        };
        let response: CreateDriveResponse = self
            .send(subjects::DRIVE_CREATE_REQUEST, &request, DRIVE_TIMEOUT)
            .await?;
        match response {
            CreateDriveResponse::Created(record) | CreateDriveResponse::AlreadyExists(record) => Ok(record),
            CreateDriveResponse::Denied(error) => Err(ClientError::DriveDenied(error)),
        }
    }

    /// Lists a tenant's drives. A refusal is [`ClientError::DriveDenied`], never an empty list.
    pub async fn list_drives(&self, tenant: TenantId) -> Result<Vec<DriveRecord>, ClientError> {
        let request = ListDrivesRequest { tenant };
        let response: ListDrivesResponse = self.send(subjects::DRIVE_LIST_REQUEST, &request, DRIVE_TIMEOUT).await?;
        match response.denied {
            Some(error) => Err(ClientError::DriveDenied(error)),
            None => Ok(response.drives),
        }
    }

    pub async fn delete_drive(&self, tenant: TenantId, drive_id: DriveId) -> Result<(), ClientError> {
        let request = DeleteDriveRequest { tenant, drive_id };
        let response: DeleteDriveResponse = self
            .send(subjects::DRIVE_DELETE_REQUEST, &request, DRIVE_TIMEOUT)
            .await?;
        match response {
            DeleteDriveResponse::Applied => Ok(()),
            DeleteDriveResponse::Denied(error) => Err(ClientError::DriveDenied(error)),
        }
    }
}
