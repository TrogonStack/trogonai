mod support;

use serde_json::json;
use support::*;
use trogon_sandbox_contract::{ReadFileRequest, ReadFileResponse, WriteFilesRequest, WriteFilesResponse};
use trogon_sandbox_daytona_worker::LeaseSandbox;

fn sandbox() -> LeaseSandbox {
    LeaseSandbox { id: SANDBOX.to_owned() }
}

#[tokio::test]
async fn writing_as_another_user_is_refused_before_any_upload() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let request: WriteFilesRequest = serde_json::from_value(json!({
        "lease_id": "lease-1",
        "files": [{"path": "a.txt", "content": [1], "mode": 420}],
        "as_user": "alice",
    }))?;
    let response = fake.exec()?.handle_write_files(&sandbox(), &request).await;
    assert!(matches!(response, WriteFilesResponse::Denied(_)), "{response:?}");
    assert_eq!(fake.count("POST", &format!("/box/{SANDBOX}/files/bulk-upload")), 0);
    assert_eq!(fake.count("POST", &format!("/box/{SANDBOX}/files/permissions")), 0);
    Ok(())
}

#[tokio::test]
async fn reading_as_another_user_is_refused_before_any_download() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let route = format!("/box/{SANDBOX}/files/download");
    fake.raw("GET", &route, 200, b"secret");
    let request: ReadFileRequest = serde_json::from_value(json!({
        "lease_id": "lease-1",
        "path": "a.txt",
        "as_user": "alice",
    }))?;
    let response = fake.exec()?.handle_read_file(&sandbox(), &request).await;
    assert!(matches!(response, ReadFileResponse::Denied(_)), "{response:?}");
    assert_eq!(fake.count("GET", &route), 0);
    Ok(())
}
