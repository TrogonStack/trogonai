//! A scripted guest: envd answers set up per request, for file operations whose failure paths a real
//! filesystem cannot produce on demand.
// Shared by two test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::error::Error;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use trogon_sandbox_contract::{Base64Data, FileOp, FileOpRequest, FileOpResponse, LeaseId};
use trogon_sandbox_e2b_worker::{E2bConfig, E2bExec, Envd, EnvdTarget, LeaseSandbox};
use url::Url;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub type TestResult = Result<(), Box<dyn Error>>;

pub struct Guest {
    server: MockServer,
    exec: E2bExec,
    sandbox: LeaseSandbox,
}

/// One envd filesystem entry.
pub fn entry(path: &str, kind: &str, size: u64) -> Value {
    let name = path.rsplit('/').next().unwrap_or(path);
    json!({"name": name, "path": path, "type": kind, "size": size.to_string()})
}

pub fn file(path: &str, size: u64) -> Value {
    entry(path, "FILE_TYPE_FILE", size)
}

pub fn directory(path: &str) -> Value {
    entry(path, "FILE_TYPE_DIRECTORY", 0)
}

pub fn symlink(path: &str, target: &str) -> Value {
    let mut value = entry(path, "FILE_TYPE_FILE", 0);
    value["isSymlink"] = true.into();
    value["symlinkTarget"] = target.into();
    value
}

/// A Connect process stream that ends with `exit_code` after writing `stderr`.
fn process_stream(exit_code: i32, stderr: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut body = Vec::new();
    for (flags, message) in [
        (0u8, json!({"event": {"start": {"pid": 1}}})),
        (
            0,
            json!({"event": {"data": {"stdout": b64(b""), "stderr": b64(stderr.as_bytes())}}}),
        ),
        (0, json!({"event": {"end": {"exited": true, "exitCode": exit_code}}})),
        (2, json!({})),
    ] {
        let payload = serde_json::to_vec(&message)?;
        body.push(flags);
        body.extend_from_slice(&u32::try_from(payload.len())?.to_be_bytes());
        body.extend(payload);
    }
    Ok(body)
}

impl Guest {
    pub async fn new() -> Result<Self, Box<dyn Error>> {
        let server = MockServer::start().await;
        let base = Url::parse(&server.uri())?;
        let config = E2bConfig::new(base.clone(), "e2b.app", "fixture-key")?.with_envd_url(base)?;
        let sandbox = LeaseSandbox {
            sandbox_id: "fixture".into(),
            target: EnvdTarget::for_sandbox(&config, "fixture", None, None)?,
            traffic_token: None,
            domain: "e2b.app".into(),
            exposed_ports: vec![],
        };
        Ok(Self {
            server,
            exec: E2bExec::new(Envd::new(Duration::from_secs(5))?),
            sandbox,
        })
    }

    pub async fn run(&self, op: FileOp) -> FileOpResponse {
        self.exec
            .handle_file_op(
                &self.sandbox,
                &FileOpRequest {
                    lease_id: LeaseId::new("lease-files"),
                    as_user: None,
                    op,
                },
            )
            .await
    }

    /// How many requests envd has received.
    pub async fn requests(&self) -> usize {
        self.server
            .received_requests()
            .await
            .map_or(0, |requests| requests.len())
    }

    async fn filesystem(&self, rpc: &str, body: Option<Value>, reply: ResponseTemplate) {
        let mock = Mock::given(method("POST")).and(path(format!("/filesystem.Filesystem/{rpc}")));
        match body {
            Some(body) => mock.and(body_json(body)),
            None => mock,
        }
        .respond_with(reply)
        .mount(&self.server)
        .await;
    }

    pub async fn stat(&self, at: &str, entry: Value) {
        self.stat_reply(at, ResponseTemplate::new(200).set_body_json(json!({"entry": entry})))
            .await;
    }

    /// Answers the first `Stat` of `at` as missing, and later ones with `entry`.
    pub async fn stat_after_creation(&self, at: &str, entry: Value) {
        Mock::given(method("POST"))
            .and(path("/filesystem.Filesystem/Stat"))
            .and(body_json(json!({"path": at})))
            .respond_with(status(404))
            .up_to_n_times(1)
            .mount(&self.server)
            .await;
        self.stat(at, entry).await;
    }

    pub async fn stat_reply(&self, at: &str, reply: ResponseTemplate) {
        self.filesystem("Stat", Some(json!({"path": at})), reply).await;
    }

    pub async fn list_dir(&self, at: &str, reply: Value) {
        self.filesystem(
            "ListDir",
            Some(json!({"path": at, "depth": 1})),
            ResponseTemplate::new(200).set_body_json(reply),
        )
        .await;
    }

    /// Answers every `Remove` with `reply`.
    pub async fn remove(&self, reply: ResponseTemplate) {
        self.filesystem("Remove", None, reply).await;
    }

    /// Answers every file upload with `reply`.
    pub async fn upload(&self, reply: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/files"))
            .respond_with(reply)
            .mount(&self.server)
            .await;
    }

    pub async fn download(&self, at: &str, bytes: &[u8]) {
        Mock::given(method("GET"))
            .and(path("/files"))
            .and(query_param("path", at))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()))
            .mount(&self.server)
            .await;
    }

    /// Answers every guest script with `exit_code` and `stderr`.
    pub async fn script(&self, exit_code: i32, stderr: &str) -> Result<(), Box<dyn Error>> {
        Mock::given(method("POST"))
            .and(path("/process.Process/Start"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/connect+json")
                    .set_body_bytes(process_stream(exit_code, stderr)?),
            )
            .mount(&self.server)
            .await;
        Ok(())
    }
}

pub fn status(code: u16) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_json(json!({"message": "scripted"}))
}

pub fn ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({}))
}

pub fn no_content() -> ResponseTemplate {
    ResponseTemplate::new(204)
}

/// The wire form of raw bytes.
pub fn payload(bytes: &[u8]) -> Result<Base64Data, Box<dyn Error>> {
    Ok(Base64Data::new(
        base64::engine::general_purpose::STANDARD.encode(bytes),
    )?)
}
