use serde_json::json;
use trogon_sandbox_contract::{LeaseId, SandboxFileWrite, WriteFilesRequest, WriteFilesResponse};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{TestResult, exited, fixture, mount_script};
use crate::exec::constants::CHMOD_SCRIPT;

#[tokio::test]
async fn an_upload_whose_mode_cannot_be_applied_is_denied() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    mount_script(&server, CHMOD_SCRIPT, exited(1)?).await;
    let (exec, sandbox) = fixture(&server)?;
    let write = WriteFilesRequest {
        lease_id: LeaseId::new("lease-1"),
        files: vec![SandboxFileWrite {
            path: "/w/a.txt".to_owned(),
            content: b"hi".to_vec(),
            mode: Some(0o755),
        }],
        as_user: None,
    };
    assert!(matches!(
        exec.handle_write_files(&sandbox, &write).await,
        WriteFilesResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn an_upload_is_written_with_or_without_a_mode() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    mount_script(&server, CHMOD_SCRIPT, exited(0)?).await;
    let (exec, sandbox) = fixture(&server)?;
    let write = WriteFilesRequest {
        lease_id: LeaseId::new("lease-1"),
        files: vec![
            SandboxFileWrite {
                path: "/w/a.txt".to_owned(),
                content: b"hi".to_vec(),
                mode: Some(0o644),
            },
            SandboxFileWrite {
                path: "/w/b.txt".to_owned(),
                content: b"hi".to_vec(),
                mode: None,
            },
        ],
        as_user: None,
    };
    assert!(matches!(
        exec.handle_write_files(&sandbox, &write).await,
        WriteFilesResponse::Written
    ));
    Ok(())
}
