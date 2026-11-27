mod admin_support;
mod common;

use std::time::Duration;

use admin_support::{drive_id, drive_record, tenant};
use common::{MockBus, TestResult, client_replying};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    CreateDriveRequest, CreateDriveResponse, DeleteDriveResponse, DriveError, DriveStorage, ListDrivesResponse,
};
use trogon_sandbox_protocol::subjects;

#[tokio::test]
async fn create_drive_returns_the_record_and_sends_an_s3_bucket() -> TestResult {
    let record = drive_record()?;
    let (client, log) = client_replying(&CreateDriveResponse::Created(record.clone()))?;

    let created = client
        .create_drive(tenant()?, record.id.clone(), Some(10), "bucket-1".into())
        .await?;

    assert_eq!(created.id, record.id);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::DRIVE_CREATE_REQUEST);
    assert_eq!(sent.timeout, Duration::from_secs(2));
    let body: CreateDriveRequest = serde_json::from_slice(&sent.payload)?;
    assert_eq!(body.storage.s3_bucket(), Some("bucket-1"));
    Ok(())
}

#[tokio::test]
async fn create_drive_treats_an_existing_drive_as_success() -> TestResult {
    let record = drive_record()?;
    let (client, _) = client_replying(&CreateDriveResponse::AlreadyExists(record.clone()))?;

    let created = client
        .create_drive_with_storage(
            tenant()?,
            record.id.clone(),
            None,
            DriveStorage::S3 {
                bucket: "bucket-1".into(),
            },
        )
        .await?;

    assert_eq!(created.id, record.id);
    Ok(())
}

#[tokio::test]
async fn create_drive_returns_drive_denied_when_the_broker_denies() -> TestResult {
    let (client, _) = client_replying(&CreateDriveResponse::Denied(DriveError::DriveAlreadyExists {
        drive_id: drive_id()?,
    }))?;

    let result = client
        .create_drive(tenant()?, drive_id()?, None, "bucket-1".into())
        .await;

    assert!(matches!(
        result,
        Err(ClientError::DriveDenied(DriveError::DriveAlreadyExists { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn list_drives_returns_the_records() -> TestResult {
    let record = drive_record()?;
    let (client, log) = client_replying(&ListDrivesResponse {
        drives: vec![record.clone()],
        denied: None,
    })?;

    let drives = client.list_drives(tenant()?).await?;

    assert_eq!(drives.len(), 1);
    assert_eq!(drives[0].id, record.id);
    assert_eq!(log.only()?.subject, subjects::DRIVE_LIST_REQUEST);
    Ok(())
}

#[tokio::test]
async fn list_drives_surfaces_a_refusal_as_denied_not_as_empty() -> TestResult {
    let (client, _) = client_replying(&ListDrivesResponse {
        drives: Vec::new(),
        denied: Some(DriveError::Unauthorized {
            reason: "wrong tenant".into(),
        }),
    })?;

    let drives = client.list_drives(tenant()?).await;

    assert!(
        matches!(drives, Err(ClientError::DriveDenied(DriveError::Unauthorized { .. }))),
        "{drives:?}"
    );
    Ok(())
}

#[tokio::test]
async fn delete_drive_succeeds_when_the_broker_applies() -> TestResult {
    let (client, log) = client_replying(&DeleteDriveResponse::Applied)?;

    client.delete_drive(tenant()?, drive_id()?).await?;

    assert_eq!(log.only()?.subject, subjects::DRIVE_DELETE_REQUEST);
    Ok(())
}

#[tokio::test]
async fn delete_drive_returns_drive_denied_when_the_broker_denies() -> TestResult {
    let (client, _) = client_replying(&DeleteDriveResponse::Denied(DriveError::DriveNotFound {
        drive_id: drive_id()?,
    }))?;

    let result = client.delete_drive(tenant()?, drive_id()?).await;

    assert!(matches!(
        result,
        Err(ClientError::DriveDenied(DriveError::DriveNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn drive_calls_time_out_when_the_bus_does_not_reply() -> TestResult {
    let result = MockBus::timing_out()
        .into_client()
        .delete_drive(tenant()?, drive_id()?)
        .await;

    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}
