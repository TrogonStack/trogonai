#![cfg(unix)]

mod support;

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use support::{TestResult, outcome, sandbox_at};
use trogon_sandbox_contract::{
    FileKind, FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, FileStat, LeaseId, SandboxUserName,
};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Deserialize)]
struct Stat {
    stat: FileStat,
}

#[derive(Deserialize)]
struct Listing {
    entries: Vec<FileStat>,
    truncated: bool,
}

#[tokio::test]
async fn stat_uses_the_native_filesystem_rpc_and_requested_user() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/Stat"))
        .and(header("Connect-Protocol-Version", "1"))
        .and(header("authorization", "Basic cm9vdDo="))
        .and(body_json(json!({"path": "/work/link"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "entry": {
                "name": "link", "path": "/work/link", "type": "FILE_TYPE_FILE",
                "isSymlink": true, "symlinkTarget": "target", "size": "17",
                "mode": 384, "modifiedTime": "2026-01-02T03:04:05Z"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &sandbox_at(&server.uri())?,
            &FileOpRequest {
                lease_id: LeaseId::new("lease-files"),
                as_user: Some(SandboxUserName::new("root")?),
                op: FileOp::Stat {
                    path: "/work/link".into(),
                },
            },
        )
        .await;
    let Stat { stat: entry } = outcome(&reply)?;
    assert_eq!(entry.path, "/work/link");
    assert_eq!(entry.name, "link");
    assert_eq!(entry.kind, FileKind::Symlink);
    assert_eq!(entry.symlink_target.as_deref(), Some("target"));
    assert_eq!(entry.size, 17);
    assert_eq!(entry.mode, Some(0o600));
    assert_eq!(entry.modified_unix_ms, Some(1767323045000));
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn list_is_relative_bounded_and_never_follows_symlink_directories() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/Stat"))
        .and(body_json(json!({"path": "/work"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"entry": {
            "name": "work", "path": "/work", "type": "FILE_TYPE_DIRECTORY"
        }})))
        .mount(&server)
        .await;
    let listing = |directory: &str, entries: serde_json::Value, calls: u64| {
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/ListDir"))
            .and(body_json(json!({"path": directory, "depth": 1})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "entries": entries })))
            .expect(calls)
    };
    listing(
        "/work",
        json!([
            {"name": "z", "path": "/work/z", "type": "FILE_TYPE_FILE"},
            {"name": "link", "path": "/work/link", "type": "FILE_TYPE_DIRECTORY", "isSymlink": true},
            {"name": "dir", "path": "/work/dir", "type": "FILE_TYPE_DIRECTORY"}
        ]),
        1,
    )
    .mount(&server)
    .await;
    listing(
        "/work/dir",
        json!([
            {"name": "b", "path": "/work/dir/b", "type": "FILE_TYPE_FILE"},
            {"name": "a", "path": "/work/dir/a", "type": "FILE_TYPE_FILE"}
        ]),
        1,
    )
    .mount(&server)
    .await;
    listing("/work/link", json!([]), 0).mount(&server).await;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &sandbox_at(&server.uri())?,
            &FileOpRequest {
                lease_id: LeaseId::new("lease-files"),
                as_user: None,
                op: FileOp::List {
                    path: "/work".into(),
                    recursive: true.into(),
                    max_entries: Some(4),
                },
            },
        )
        .await;
    let Listing { entries, truncated } = outcome(&reply)?;
    assert!(truncated);
    assert_eq!(
        entries.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
        ["dir", "dir/a", "dir/b", "link"]
    );
    assert_eq!(entries[3].kind, FileKind::Symlink);
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn list_refuses_entries_outside_the_requested_directory() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/Stat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"entry": {
            "name": "work", "path": "/work", "type": "FILE_TYPE_DIRECTORY"
        }})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/filesystem.Filesystem/ListDir"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"entries": [
            {"name": "elsewhere", "path": "/elsewhere", "type": "FILE_TYPE_DIRECTORY"}
        ]})))
        .mount(&server)
        .await;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &sandbox_at(&server.uri())?,
            &FileOpRequest {
                lease_id: LeaseId::new("lease-files"),
                as_user: None,
                op: FileOp::List {
                    path: "/work".into(),
                    recursive: true.into(),
                    max_entries: None,
                },
            },
        )
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::Io,
                ..
            }
        ),
        "{reply:?}"
    );
    Ok(())
}
