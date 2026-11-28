use super::*;
use crate::constants::{DEFAULT_DRIVE_SIZE_GIB, MAX_DRIVE_SIZE_GIB};

impl BrokerCore {
    /// Creates a new drive for `req.tenant`. `size_gib` defaults to
    /// `DEFAULT_DRIVE_SIZE_GIB` when unset and is denied when it exceeds
    /// `MAX_DRIVE_SIZE_GIB`.
    pub async fn handle_create_drive(&self, req: CreateDriveRequest) -> CreateDriveResponse {
        if req.drive_id.tenant != req.tenant {
            return CreateDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id });
        }
        let size_gib = req.size_gib.unwrap_or(DEFAULT_DRIVE_SIZE_GIB);

        if size_gib > MAX_DRIVE_SIZE_GIB {
            warn!(
                tenant = %req.tenant,
                requested_gib = size_gib,
                max_gib = MAX_DRIVE_SIZE_GIB,
                "create drive denied: size exceeds max"
            );
            return CreateDriveResponse::Denied(DriveError::DriveSizeExceeded {
                requested_gib: size_gib,
                max_gib: MAX_DRIVE_SIZE_GIB,
            });
        }

        let created = match req.storage {
            trogon_sandbox_contract::DriveStorage::S3 { bucket } => {
                self.drive_store
                    .create(&req.tenant, &req.drive_id, size_gib, bucket)
                    .await
            }
            trogon_sandbox_contract::DriveStorage::ProviderVolume { provider_volume } => {
                self.drive_store
                    .create_provider_volume(&req.tenant, &req.drive_id, size_gib, provider_volume)
                    .await
            }
        };
        match created {
            Ok(record) => {
                info!(tenant = %req.tenant, drive = %req.drive_id.name, "drive created");
                CreateDriveResponse::Created(record)
            }
            Err(DriveStoreError::AlreadyExists) => match self.drive_store.get(&req.drive_id).await {
                Ok(Some(existing)) => {
                    info!(tenant = %req.tenant, drive = %req.drive_id.name, "create drive: already exists, returning existing record");
                    CreateDriveResponse::AlreadyExists(existing)
                }
                Ok(None) | Err(_) => {
                    warn!(tenant = %req.tenant, drive = %req.drive_id.name, "create drive denied: already exists but existing record could not be fetched (race)");
                    CreateDriveResponse::Denied(DriveError::DriveAlreadyExists { drive_id: req.drive_id })
                }
            },
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, drive = %req.drive_id.name, "create drive denied: drive store error");
                CreateDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id })
            }
        }
    }

    /// Lists all drives owned by `req.tenant`.
    pub async fn handle_list_drives(&self, req: ListDrivesRequest) -> ListDrivesResponse {
        match self.drive_store.list(&req.tenant).await {
            Ok(drives) => ListDrivesResponse { denied: None, drives },
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, "list drives denied: drive store error");
                ListDrivesResponse {
                    denied: Some(DriveError::StorageBackend { message: e.to_string() }),
                    drives: Vec::new(),
                }
            }
        }
    }

    /// Deletes a drive owned by `req.tenant`. Denied when the drive does not
    /// exist or is currently attached to a lease (tracked via
    /// `DriveRecord::attached_lease`, kept in sync by lease grant/release).
    pub async fn handle_delete_drive(&self, req: DeleteDriveRequest) -> DeleteDriveResponse {
        if req.tenant != req.drive_id.tenant {
            return DeleteDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id });
        }
        let record = match self.drive_store.get(&req.drive_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(tenant = %req.tenant, drive = %req.drive_id.name, "delete drive denied: not found");
                return DeleteDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id });
            }
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, drive = %req.drive_id.name, "delete drive denied: drive store lookup failed");
                return DeleteDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id });
            }
        };

        if record.tenant != req.tenant || record.id != req.drive_id {
            return DeleteDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id });
        }

        if let Some(lease_id) = record.attached_lease.clone() {
            warn!(
                tenant = %req.tenant,
                drive = %req.drive_id.name,
                lease_id = %lease_id,
                "delete drive denied: drive is attached"
            );
            return DeleteDriveResponse::Denied(DriveError::DriveAttached {
                drive_id: req.drive_id,
                lease_id,
            });
        }

        if let Err(e) = self.drive_data_purger.purge(&record).await {
            warn!(error = %e, drive = %req.drive_id.name, "delete drive denied: storage purge failed");
            return DeleteDriveResponse::Denied(DriveError::StorageBackend { message: e.to_string() });
        }

        match self.drive_store.delete(&req.drive_id).await {
            Ok(()) => {
                info!(tenant = %req.tenant, drive = %req.drive_id.name, "drive deleted");
                DeleteDriveResponse::Applied
            }
            Err(DriveStoreError::NotFound) => {
                warn!(tenant = %req.tenant, drive = %req.drive_id.name, "delete drive denied: not found");
                DeleteDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id })
            }
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, drive = %req.drive_id.name, "delete drive denied: drive store error");
                DeleteDriveResponse::Denied(DriveError::DriveNotFound { drive_id: req.drive_id })
            }
        }
    }
}
