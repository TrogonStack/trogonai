#![cfg(unix)]

mod support;

use std::time::Duration;

use serde_json::json;
use support::{TestResult, data, sandbox_at};
use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request(op: FileOp) -> FileOpRequest {
    FileOpRequest {
        lease_id: LeaseId::new("lease-files"),
        as_user: None,
        op,
    }
}

#[tokio::test]
async fn ranged_reads_preserve_binary_data_and_report_bounds() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/Stat"))
        .and(body_json(json!({"path": "/work/data"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"entry": {
            "name": "data", "path": "/work/data", "type": "FILE_TYPE_FILE", "size": "10"
        }})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(query_param("path", "/work/data"))
        .and(query_param("username", "user"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9]))
        .expect(2)
        .mount(&server)
        .await;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let target = sandbox_at(&server.uri())?;
    for (offset, length, max_bytes, expected, eof, truncated) in [
        (2, Some(6), Some(3), vec![2, 3, 4], false, true),
        (8, None, None, vec![8, 9], true, false),
        (u64::MAX, None, None, vec![], true, false),
        (0, None, Some(0), vec![], false, true),
    ] {
        let reply = exec
            .handle_file_op(
                &target,
                &request(FileOp::Read {
                    path: "/work/data".into(),
                    offset,
                    length,
                    max_bytes,
                }),
            )
            .await;
        assert_eq!(
            reply,
            FileOpResponse::Content {
                data: data(&expected)?,
                offset,
                total_size: 10,
                eof: eof.into(),
                truncated: truncated.into(),
            }
        );
    }
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn ranged_reads_preserve_missing_and_directory_errors() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/Stat"))
        .and(body_json(json!({"path": "/missing"})))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"code": "not_found", "message": "missing"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/Stat"))
        .and(body_json(json!({"path": "/directory"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"entry": {
            "name": "directory", "path": "/directory", "type": "FILE_TYPE_DIRECTORY"
        }})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let target = sandbox_at(&server.uri())?;
    let read = |path: &str| {
        request(FileOp::Read {
            path: path.into(),
            offset: 0,
            length: None,
            max_bytes: None,
        })
    };
    assert_eq!(
        exec.handle_file_op(&target, &read("/missing")).await,
        FileOpResponse::NotFound {
            path: "/missing".into()
        }
    );
    assert!(matches!(
        exec.handle_file_op(&target, &read("/directory")).await,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::IsADirectory,
            ..
        }
    ));
    server.verify().await;
    Ok(())
}
