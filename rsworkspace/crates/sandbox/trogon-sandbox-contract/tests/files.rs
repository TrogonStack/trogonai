use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    LeaseError, LeaseId, ReadFileRequest, ReadFileResponse, SandboxFileWrite, SandboxUserName, WriteFilesRequest,
    WriteFilesResponse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn roundtrip<T>(value: &T, expected: Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value)?, expected);
    assert_eq!(&serde_json::from_value::<T>(expected)?, value);
    Ok(())
}

#[test]
fn write_files_request_round_trips_with_and_without_optional_fields() -> TestResult {
    let bare = WriteFilesRequest {
        lease_id: LeaseId::new("lease-1"),
        files: vec![SandboxFileWrite {
            path: "/f".to_string(),
            content: vec![1, 2],
            mode: None,
        }],
        as_user: None,
    };
    roundtrip(
        &bare,
        json!({"lease_id": "lease-1", "files": [{"path": "/f", "content": [1, 2]}]}),
    )?;

    let full = WriteFilesRequest {
        lease_id: LeaseId::new("lease-1"),
        files: vec![SandboxFileWrite {
            path: "f".to_string(),
            content: Vec::new(),
            mode: Some(0o644),
        }],
        as_user: Some(SandboxUserName::new("dev")?),
    };
    roundtrip(
        &full,
        json!({
            "lease_id": "lease-1",
            "files": [{"path": "f", "content": [], "mode": 420}],
            "as_user": "dev",
        }),
    )
}

#[test]
fn write_files_request_rejects_an_invalid_user() -> TestResult {
    let error =
        serde_json::from_value::<WriteFilesRequest>(json!({"lease_id": "l", "files": [], "as_user": "Bad User"}))
            .err()
            .ok_or("an invalid user must be rejected")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn write_files_response_is_tagged_by_outcome() -> TestResult {
    roundtrip(&WriteFilesResponse::Written, json!({"outcome": "written"}))?;
    roundtrip(
        &WriteFilesResponse::Denied(LeaseError::NoProviderAvailable),
        json!({"outcome": "denied", "kind": "no_provider_available"}),
    )
}

#[test]
fn read_file_request_round_trips_with_and_without_a_user() -> TestResult {
    let bare = ReadFileRequest {
        lease_id: LeaseId::new("l"),
        path: "/etc/hosts".to_string(),
        as_user: None,
    };
    roundtrip(&bare, json!({"lease_id": "l", "path": "/etc/hosts"}))?;
    let as_user = ReadFileRequest {
        as_user: Some(SandboxUserName::new("root")?),
        ..bare
    };
    roundtrip(
        &as_user,
        json!({"lease_id": "l", "path": "/etc/hosts", "as_user": "root"}),
    )
}

#[test]
fn read_file_response_is_tagged_by_outcome() -> TestResult {
    roundtrip(
        &ReadFileResponse::Content { data: vec![104, 105] },
        json!({"outcome": "content", "data": [104, 105]}),
    )?;
    roundtrip(&ReadFileResponse::NotFound, json!({"outcome": "not_found"}))?;
    roundtrip(
        &ReadFileResponse::Denied(LeaseError::NoProviderAvailable),
        json!({"outcome": "denied", "kind": "no_provider_available"}),
    )
}
