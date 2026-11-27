mod support;

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use support::{Reply, Server, TestResult};
use trogon_sandbox_contract::{
    GetSandboxDomainResponse, LeaseError, LeaseId, ReadFileRequest, ReadFileResponse, RunCommandRequest,
    RunCommandResponse, SandboxFileWrite, WriteFilesRequest, WriteFilesResponse, WriteStdinResponse,
};
use trogon_sandbox_vercel_worker::LeaseSandbox;

fn sandbox(session_id: &str) -> LeaseSandbox {
    LeaseSandbox {
        name: "sbx-a".to_owned(),
        session_id: session_id.to_owned(),
        routes: BTreeMap::from([(3000, "https://sbx-a-3000.example.test/".to_owned())]),
    }
}

fn run_request(change: impl FnOnce(&mut Value)) -> Result<RunCommandRequest, serde_json::Error> {
    let mut wire = json!({"lease_id": "lease-1", "command": ["true"]});
    change(&mut wire);
    serde_json::from_value(wire)
}

fn write_request(path: &str, as_user: Option<&str>) -> Result<WriteFilesRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "lease_id": "lease-1",
        "files": [{"path": path, "content": [104, 101, 108, 108, 111], "mode": 493}],
        "as_user": as_user,
    }))
}

fn read_request(path: &str, as_user: Option<&str>) -> Result<ReadFileRequest, serde_json::Error> {
    serde_json::from_value(json!({"lease_id": "lease-1", "path": path, "as_user": as_user}))
}

type Unpacked = (String, String, u32);

fn unpacked(body: &[u8]) -> Result<Vec<Unpacked>, Box<dyn std::error::Error + Send + Sync>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(body));
    let mut files = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let mut content = String::new();
        entry.read_to_string(&mut content)?;
        files.push((
            entry.path()?.to_string_lossy().into_owned(),
            content,
            entry.header().mode()?,
        ));
    }
    Ok(files)
}

#[tokio::test]
async fn options_the_worker_cannot_serve_are_refused_without_a_call() -> TestResult {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let server = Server::start(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Reply::json(200, json!({}))
    })
    .await?;
    let exec = server.exec()?;
    let refused = [
        run_request(|wire| wire["stream_output"] = json!(true))?,
        run_request(|wire| wire["run_as_user"] = json!("someone"))?,
        run_request(|wire| wire["command"] = json!([]))?,
        run_request(|wire| wire["require_launch_cancellation"] = json!(true))?,
    ];
    for request in &refused {
        let response = exec.handle_run_command(&sandbox("sess_1"), request).await;
        assert!(matches!(
            response,
            RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(matches!(exec.handle_write_stdin(), WriteStdinResponse::Unsupported));
    Ok(())
}

#[tokio::test]
async fn written_files_travel_as_a_gzip_tar_with_their_mode() -> TestResult {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let server = Server::start(move |request| {
        let content_type = request.headers.get("content-type").cloned();
        let cwd = request.headers.get("x-cwd").cloned();
        recorded.lock().unwrap_or_else(PoisonError::into_inner).push((
            request.route().to_owned(),
            content_type,
            cwd,
            request.body,
        ));
        Reply::json(200, json!({}))
    })
    .await?;
    let response = server
        .exec()?
        .handle_write_files(&sandbox("sess_1"), &write_request("/work/a.txt", None)?)
        .await;
    assert!(matches!(response, WriteFilesResponse::Written));
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
    let (route, content_type, cwd, body) = seen.first().ok_or("no request")?;
    assert_eq!(route, "/v2/sandboxes/sessions/sess_1/fs/write");
    assert_eq!(content_type.as_deref(), Some("application/gzip"));
    assert_eq!(cwd.as_deref(), Some("/"));
    assert_eq!(unpacked(body)?, [("work/a.txt".to_owned(), "hello".to_owned(), 0o755)]);
    Ok(())
}

#[tokio::test]
async fn a_write_that_cannot_be_served_is_denied() -> TestResult {
    let server = Server::start(|_| Reply::json(500, json!({"error": {"message": "down"}}))).await?;
    let exec = server.exec()?;
    let denied = |response| matches!(response, WriteFilesResponse::Denied(_));
    assert!(denied(
        exec.handle_write_files(&sandbox("sess_1"), &write_request("/a", Some("root"))?)
            .await
    ));
    assert!(denied(
        exec.handle_write_files(&sandbox("sess_1"), &write_request("/", None)?)
            .await
    ));
    assert!(denied(
        exec.handle_write_files(&sandbox("../x"), &write_request("/a", None)?)
            .await
    ));
    assert!(matches!(
        exec.handle_write_files(&sandbox("sess_1"), &write_request("/a", None)?)
            .await,
        WriteFilesResponse::Denied(LeaseError::Backend { .. })
    ));
    let empty = WriteFilesRequest {
        lease_id: LeaseId::new("lease-1"),
        files: vec![SandboxFileWrite {
            path: String::new(),
            content: Vec::new(),
            mode: None,
        }],
        as_user: None,
    };
    assert!(denied(exec.handle_write_files(&sandbox("sess_1"), &empty).await));
    Ok(())
}

#[tokio::test]
async fn read_returns_the_bytes_and_a_missing_file_is_not_found() -> TestResult {
    let server = Server::start(|request| match request.json() {
        Ok(body) if body == json!({"path": "/etc/hostname", "cwd": "/"}) => Reply::Complete(200, b"box\n".to_vec()),
        Ok(body) if body == json!({"path": "/nope", "cwd": "/"}) => {
            Reply::json(404, json!({"error": {"message": "no such file"}}))
        }
        _ => Reply::json(500, json!({})),
    })
    .await?;
    let exec = server.exec()?;
    assert!(matches!(
        exec.handle_read_file(&sandbox("sess_1"), &read_request("/etc/hostname", None)?).await,
        ReadFileResponse::Content { data } if data == b"box\n"
    ));
    assert!(matches!(
        exec.handle_read_file(&sandbox("sess_1"), &read_request("/nope", None)?)
            .await,
        ReadFileResponse::NotFound
    ));
    Ok(())
}

#[tokio::test]
async fn a_read_that_cannot_be_served_is_denied() -> TestResult {
    let server = Server::start(|_| Reply::json(500, json!({"error": {"message": "down"}}))).await?;
    let exec = server.exec()?;
    let denied = |response| matches!(response, ReadFileResponse::Denied(_));
    assert!(denied(
        exec.handle_read_file(&sandbox("sess_1"), &read_request("/a", Some("root"))?)
            .await
    ));
    assert!(denied(
        exec.handle_read_file(&sandbox("../x"), &read_request("/a", None)?)
            .await
    ));
    assert!(denied(
        exec.handle_read_file(&sandbox("sess_1"), &read_request("/a", None)?)
            .await
    ));
    Ok(())
}

#[tokio::test]
async fn only_declared_ports_have_a_domain() -> TestResult {
    let server = Server::start(|_| Reply::json(200, json!({}))).await?;
    let exec = server.exec()?;
    let GetSandboxDomainResponse::Available { endpoint, access, .. } = exec.handle_get_domain(&sandbox("sess_1"), 3000)
    else {
        return Err("port 3000 is declared".into());
    };
    assert_eq!(endpoint, "sbx-a-3000.example.test:443");
    assert_eq!(access.ok_or("access")?.base_url, "https://sbx-a-3000.example.test");
    assert!(matches!(
        exec.handle_get_domain(&sandbox("sess_1"), 8080),
        GetSandboxDomainResponse::PortNotExposed
    ));
    assert!(matches!(
        exec.handle_get_domain(&sandbox("sess_1"), 0),
        GetSandboxDomainResponse::Denied(_)
    ));
    let bare = LeaseSandbox {
        routes: BTreeMap::from([(80, "bare.example.test".to_owned())]),
        ..sandbox("sess_1")
    };
    assert!(matches!(
        exec.handle_get_domain(&bare, 80),
        GetSandboxDomainResponse::Available { ref endpoint, .. } if endpoint == "bare.example.test:443"
    ));
    Ok(())
}
