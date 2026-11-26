//! Constructors shared by the admin area tests.

#![allow(dead_code)]

use std::time::SystemTime;

use trogon_sandbox_contract::{Digest, DriveId, DriveName, DriveRecord, DriveStorage, LeaseError, TenantId};

use crate::common::TestResult;

pub fn unauthorized() -> LeaseError {
    LeaseError::Unauthorized {
        reason: "wrong tenant".into(),
    }
}

pub fn digest() -> Result<Digest, trogon_sandbox_contract::DigestError> {
    Digest::new(format!("sha256:{}", "a".repeat(64)))
}

pub fn tenant() -> Result<TenantId, trogon_sandbox_contract::TenantIdError> {
    TenantId::new("tenant-1")
}

pub fn drive_id() -> Result<DriveId, Box<dyn std::error::Error>> {
    Ok(DriveId {
        tenant: tenant()?,
        name: DriveName::new("drive-1")?,
    })
}

pub fn drive_record() -> Result<DriveRecord, Box<dyn std::error::Error>> {
    Ok(DriveRecord {
        id: drive_id()?,
        tenant: tenant()?,
        size_gib: 10,
        created_at: SystemTime::now(),
        attached_lease: None,
        storage: DriveStorage::S3 {
            bucket: "bucket-1".into(),
        },
    })
}

pub fn ok() -> TestResult {
    Ok(())
}
