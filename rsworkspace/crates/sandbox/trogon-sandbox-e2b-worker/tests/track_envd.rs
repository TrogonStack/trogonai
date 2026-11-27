//! Change tracking driven over NATS end to end against an in-process fake envd: the
//! lease-start baseline, checkpoints, content hashing, and the explicit size bounds.
//!
//! The fake speaks the `process.Process` `Start` Connect stream of E2B's public
//! `spec/envd/process/process.proto` and plays the manifest and hash scripts the worker runs
//! (`find | stat`, `sha256sum`) against an in-memory file tree, honoring the line and byte
//! ceilings the scripts pass to `head`. It is NOT a live E2B sandbox; nothing here proves
//! behaviour against the real service.

mod support;

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_nats::Client;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use support::{Boxed, TestResult, outcome};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId, LeaseRequest, ProviderKind, WorkerId,
};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{
    BASELINE_PATH, E2bApi, E2bConfig, E2bExec, E2bWorkerError, E2bWorkerProvider, Envd, MAX_MANIFEST_BYTES, run,
};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, Provider, nats_transfer, subjects};
use url::Url;

#[derive(Clone)]
struct Node {
    dir: bool,
    size: u64,
    mtime: String,
    digest: String,
}

#[derive(Default)]
struct Fake {
    tree: BTreeMap<String, Node>,
    clock: u64,
    /// Every script invocation: (user, path, recursive flag, line ceiling, byte ceiling).
    scans: Vec<(String, String, String, String, String)>,
    /// When set, a scan of this path exits 1 with a message on stderr.
    broken: Option<String>,
}

type Shared = Arc<Mutex<Fake>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// A stable 64-digit hex stand-in for the content's SHA-256.
fn digest_of(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:064x}", hasher.finish())
}

impl Fake {
    fn tick(&mut self) -> String {
        self.clock += 1;
        format!("2026-01-01 00:00:00.{:09} +0000", self.clock)
    }

    fn dir(&mut self, path: &str) {
        let mtime = self.tick();
        self.tree.insert(
            path.into(),
            Node {
                dir: true,
                size: 4096,
                mtime,
                digest: String::new(),
            },
        );
    }

    /// Writes a file. A rewrite keeps `mtime` when `keep_mtime`, as `touch -r` would.
    fn file(&mut self, path: &str, content: &str, keep_mtime: bool) {
        let mtime = match self.tree.get(path) {
            Some(old) if keep_mtime => old.mtime.clone(),
            _ => self.tick(),
        };
        self.tree.insert(
            path.into(),
            Node {
                dir: false,
                size: content.len() as u64,
                mtime,
                digest: digest_of(content),
            },
        );
    }

    fn remove(&mut self, path: &str) {
        self.tree.remove(path);
    }

    fn listing(&self, path: &str, recursive: bool) -> Option<Vec<(&String, &Node)>> {
        self.tree.get(path)?;
        let prefix = format!("{path}/");
        Some(
            self.tree
                .iter()
                .filter(|(p, _)| {
                    p.strip_prefix(&prefix)
                        .is_some_and(|rest| recursive || !rest.contains('/'))
                })
                .collect(),
        )
    }
}

fn frame(flag: u8, value: &Value) -> Result<Vec<u8>, StatusCode> {
    let body = serde_json::to_vec(value).map_err(internal)?;
    let mut out = vec![flag];
    out.extend_from_slice(&u32::try_from(body.len()).map_err(internal)?.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn stream(stdout: &[u8], stderr: &str, exit: i32) -> Result<Response, StatusCode> {
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let mut body = frame(0, &json!({"event": {"start": {"pid": 1}}}))?;
    if !stdout.is_empty() {
        body.extend(frame(0, &json!({"event": {"data": {"stdout": b64(stdout)}}}))?);
    }
    if !stderr.is_empty() {
        body.extend(frame(
            0,
            &json!({"event": {"data": {"stderr": b64(stderr.as_bytes())}}}),
        )?);
    }
    let end = if exit == 0 {
        json!({"exited": true, "status": "exit status 0"})
    } else {
        json!({"exited": true, "exitCode": exit, "status": "exit"})
    };
    body.extend(frame(0, &json!({"event": {"end": end}}))?);
    // End-of-stream trailer frame: flag 0x02.
    body.extend(frame(2, &json!({}))?);
    Ok(([(CONTENT_TYPE, "application/connect+json")], body).into_response())
}

/// Applies `head -n lines | head -c bytes` to `text`.
fn head(text: String, lines: usize, bytes: usize) -> Vec<u8> {
    let kept: String = text.split_inclusive('\n').take(lines).collect();
    kept.into_bytes().into_iter().take(bytes).collect()
}

async fn start(State(fake): State<Shared>, body: axum::body::Bytes) -> Result<Response, StatusCode> {
    // A Connect request frame: one flag byte, a big-endian length, then the JSON.
    let req: Value = serde_json::from_slice(body.get(5..).ok_or(StatusCode::BAD_REQUEST)?).map_err(internal)?;
    let args: Vec<String> = req["process"]["args"]
        .as_array()
        .ok_or(StatusCode::BAD_REQUEST)?
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    // ["-l", "-c", "exec \"$@\"", "bash", "bash", "-c", <script>, "bash", path, flag, n, b]
    let arg = |index: usize| args.get(index).ok_or(StatusCode::BAD_REQUEST);
    let (script, path, recursive, lines, bytes) = (arg(6)?, arg(8)?, arg(9)?, arg(10)?, arg(11)?);
    let mut f = locked(&fake);
    f.scans.push((
        "user".to_string(),
        path.clone(),
        recursive.clone(),
        lines.clone(),
        bytes.clone(),
    ));
    if f.broken.as_deref() == Some(path.as_str()) {
        return stream(b"", "boom", 1);
    }
    let (lines, bytes) = (lines.parse().map_err(internal)?, bytes.parse().map_err(internal)?);
    let Some(found) = f.listing(path, recursive == "1") else {
        return stream(b"", "", 44);
    };
    let text: String = if script.contains("sha256sum") {
        found
            .iter()
            .filter(|(_, n)| !n.dir)
            .map(|(p, n)| format!("{}  {p}\n", n.digest))
            .collect()
    } else {
        found
            .iter()
            .map(|(p, n)| {
                let kind = if n.dir { "directory" } else { "regular file" };
                format!("{kind}|{}|{}|{p}\n", n.size, n.mtime)
            })
            .collect()
    };
    stream(&head(text, lines, bytes), "", 0)
}

async fn create_sandbox() -> impl IntoResponse {
    (
        StatusCode::CREATED,
        axum::Json(json!({
            "sandboxID": "sbx1",
            "templateID": "base",
            "envdAccessToken": "envd-tok",
        })),
    )
}

async fn delete_sandbox() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

struct Rig {
    _nats: JetStreamTestServer,
    client: Client,
    fake: Shared,
    cancel: CancellationToken,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn lease_request() -> Result<LeaseRequest, Boxed> {
    Ok(serde_json::from_value(json!({
        "tenant": "t",
        "provider_pref": ["e2b"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "base"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 600,
        "idempotency_key": "k",
        "persistent": true,
    }))?)
}

/// Starts a worker over a fake sandbox holding `seed`, then leases it.
async fn rig(seed: impl FnOnce(&mut Fake)) -> Result<Rig, Boxed> {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let mut initial = Fake::default();
    seed(&mut initial);
    let fake: Shared = Arc::new(Mutex::new(initial));
    let app = Router::new()
        .route("/v2/sandboxes", post(create_sandbox))
        .route("/sandboxes/{id}", delete(delete_sandbox))
        .route("/process.Process/Start", post(start))
        .with_state(Arc::clone(&fake));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = E2bConfig::new(origin.clone(), "e2b.test", "api-key")?.with_envd_url(origin)?;
    let api = E2bApi::new(config, Duration::from_secs(30))?;
    let envd = Envd::new(Duration::from_secs(30))?;
    let exec = Arc::new(E2bExec::new(envd.clone()));
    let provider = Arc::new(E2bWorkerProvider::new(api, "base", exec));
    provider.lease(&lease_request()?, &LeaseId::new("lease-1")).await?;

    let shell = Arc::new(E2bGuestShell::new(Arc::clone(&provider), envd));
    let terminals = Arc::new(E2bTerminals::new(shell, client.clone()));
    let cancel = CancellationToken::new();
    let worker_id = WorkerId::new("worker-1")?;
    let worker: tokio::task::JoinHandle<Result<(), E2bWorkerError>> = tokio::spawn({
        let (client, provider, cancel) = (client.clone(), provider.clone(), cancel.clone());
        async move {
            run(
                client.clone(),
                async_nats::jetstream::new(client),
                provider,
                &worker_id,
                cancel,
                terminals,
                AuthorityConfig::new(AuthorityMode::Off),
            )
            .await
        }
    });
    drop(worker);
    Ok(Rig {
        _nats: nats,
        client,
        fake,
        cancel,
    })
}

async fn file_op(client: &Client, op: FileOp) -> Result<FileOpResponse, Boxed> {
    let req = FileOpRequest {
        lease_id: LeaseId::new("lease-1"),
        op,
        as_user: None,
    };
    let subject = subjects::provider_file_op(ProviderKind::E2b, &req.lease_id);
    let body = serde_json::to_vec(&req)?;
    // The verb handlers subscribe in spawned tasks; retry until one is listening.
    let reply = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            if let Ok(reply) =
                nats_transfer::request(client, &subject, body.clone().into(), None, Duration::from_secs(60)).await
            {
                break reply;
            }
        }
    })
    .await?;
    Ok(serde_json::from_slice(&reply)?)
}

async fn changes(rig: &Rig, since: Option<&str>, path: Option<&str>) -> Result<FileOpResponse, Boxed> {
    file_op(
        &rig.client,
        FileOp::Changes {
            since: since.map(Into::into),
            path: path.map(Into::into),
        },
    )
    .await
}

async fn checkpoint(rig: &Rig, path: &str, hash: bool) -> Result<FileOpResponse, Boxed> {
    file_op(
        &rig.client,
        FileOp::Checkpoint {
            path: path.into(),
            hash: hash.into(),
        },
    )
    .await
}

type Lists = (String, Vec<String>, Vec<String>, Vec<String>);

#[derive(Deserialize)]
struct Changed {
    since: String,
    added: Vec<String>,
    modified: Vec<String>,
    deleted: Vec<String>,
    truncated: bool,
}

fn lists(response: &FileOpResponse) -> Result<Lists, Boxed> {
    let changed: Changed = outcome(response)?;
    assert!(!changed.truncated);
    Ok((changed.since, changed.added, changed.modified, changed.deleted))
}

#[derive(Deserialize)]
struct Checkpointed {
    checkpoint_id: String,
    entries: u64,
}

#[derive(Deserialize)]
struct Failure {
    kind: FileOpErrorKind,
    message: String,
}

fn failed(response: &FileOpResponse) -> Result<(FileOpErrorKind, String), Boxed> {
    let failure: Failure = outcome(response)?;
    Ok((failure.kind, failure.message))
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn home_seed(f: &mut Fake) {
    f.dir(BASELINE_PATH);
    f.dir("/home/user/src");
    f.file("/home/user/src/main.rs", "fn main() {}", false);
    f.file("/home/user/keep.txt", "keep", false);
    f.file("/home/user/edit.txt", "before", false);
    f.file("/home/user/gone.txt", "bye", false);
}

#[tokio::test]
async fn changes_since_the_lease_start_are_exact() -> TestResult {
    let rig = rig(home_seed).await?;
    {
        let mut f = locked(&rig.fake);
        f.file("/home/user/edit.txt", "after!", false);
        f.remove("/home/user/gone.txt");
        f.file("/home/user/new.txt", "new", false);
        f.dir("/home/user/newdir");
        f.file("/home/user/newdir/inner.txt", "x", false);
    }
    let (since, added, modified, deleted) = lists(&changes(&rig, None, None).await?)?;
    assert_eq!(since, FileOpResponse::CHANGES_SINCE_START);
    assert_eq!(
        added,
        strings(&["/home/user/new.txt", "/home/user/newdir", "/home/user/newdir/inner.txt"])
    );
    assert_eq!(modified, strings(&["/home/user/edit.txt"]));
    assert_eq!(deleted, strings(&["/home/user/gone.txt"]));

    // `path` narrows the report to one subtree.
    let (_, added, modified, deleted) = lists(&changes(&rig, None, Some("/home/user/newdir")).await?)?;
    assert_eq!(added, strings(&["/home/user/newdir", "/home/user/newdir/inner.txt"]));
    assert!(modified.is_empty() && deleted.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_baseline_scan_runs_once_with_the_documented_bounds() -> TestResult {
    let rig = rig(home_seed).await?;
    let f = locked(&rig.fake);
    assert_eq!(
        f.scans,
        vec![(
            "user".to_string(),
            BASELINE_PATH.to_string(),
            "1".to_string(),
            (FileOp::TRACKING_MAX_ENTRIES + 1).to_string(),
            (MAX_MANIFEST_BYTES + 1).to_string(),
        )]
    );
    Ok(())
}

#[tokio::test]
async fn an_untouched_tree_reports_nothing() -> TestResult {
    let rig = rig(home_seed).await?;
    let (_, added, modified, deleted) = lists(&changes(&rig, None, None).await?)?;
    assert!(added.is_empty() && modified.is_empty() && deleted.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_checkpoint_diffs_its_own_tree_and_hashing_sees_a_silent_rewrite() -> TestResult {
    let rig = rig(|f| {
        home_seed(f);
        f.dir("/work");
        f.file("/work/a.txt", "aaaa", false);
        f.file("/work/b.txt", "bbbb", false);
    })
    .await?;
    let plain: Checkpointed = outcome(&checkpoint(&rig, "/work", false).await?)?;
    assert_eq!(plain.entries, 2);
    let hashed: Checkpointed = outcome(&checkpoint(&rig, "/work", true).await?)?;
    {
        let mut f = locked(&rig.fake);
        // Same size, same mtime, different bytes: only the content hash can tell.
        f.file("/work/a.txt", "AAAA", true);
        f.file("/work/c.txt", "cccc", false);
        f.remove("/work/b.txt");
    }
    let (since, added, modified, deleted) = lists(&changes(&rig, Some(&plain.checkpoint_id), None).await?)?;
    assert_eq!(since, plain.checkpoint_id);
    assert_eq!(added, strings(&["/work/c.txt"]));
    assert!(modified.is_empty());
    assert_eq!(deleted, strings(&["/work/b.txt"]));

    let (_, added, modified, deleted) = lists(&changes(&rig, Some(&hashed.checkpoint_id), None).await?)?;
    assert_eq!(added, strings(&["/work/c.txt"]));
    assert_eq!(modified, strings(&["/work/a.txt"]));
    assert_eq!(deleted, strings(&["/work/b.txt"]));

    let (kind, _) = failed(&changes(&rig, Some("cp-nope"), None).await?)?;
    assert_eq!(kind, FileOpErrorKind::UnknownCheckpoint);
    Ok(())
}

#[tokio::test]
async fn a_missing_or_relative_path_is_refused_by_name() -> TestResult {
    let rig = rig(home_seed).await?;
    assert_eq!(
        checkpoint(&rig, "/nope", false).await?,
        FileOpResponse::NotFound { path: "/nope".into() }
    );
    let (kind, _) = failed(&checkpoint(&rig, "rel", false).await?)?;
    assert_eq!(kind, FileOpErrorKind::InvalidPath);
    let (kind, _) = failed(&checkpoint(&rig, "/", false).await?)?;
    assert_eq!(kind, FileOpErrorKind::InvalidPath);
    Ok(())
}

#[tokio::test]
async fn a_tree_past_the_entry_ceiling_is_refused_not_truncated() -> TestResult {
    let rig = rig(|f| {
        home_seed(f);
        f.dir("/big");
        for i in 0..=FileOp::TRACKING_MAX_ENTRIES {
            f.tree.insert(
                format!("/big/f{i:06}"),
                Node {
                    dir: false,
                    size: 1,
                    mtime: "t".into(),
                    digest: String::new(),
                },
            );
        }
    })
    .await?;
    let (kind, message) = failed(&checkpoint(&rig, "/big", false).await?)?;
    assert_eq!(kind, FileOpErrorKind::TooLarge);
    assert!(message.contains("/big"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_listing_past_the_byte_ceiling_is_refused_not_truncated() -> TestResult {
    let rig = rig(|f| {
        home_seed(f);
        f.dir("/wide");
        // Three entries with 6 MiB names: few entries, but more listing than the ceiling.
        for i in 0..3 {
            let name = format!("/wide/{i}{}", "n".repeat(6 * 1024 * 1024));
            f.tree.insert(
                name,
                Node {
                    dir: false,
                    size: 1,
                    mtime: "t".into(),
                    digest: String::new(),
                },
            );
        }
    })
    .await?;
    let (kind, message) = failed(&checkpoint(&rig, "/wide", false).await?)?;
    assert_eq!(kind, FileOpErrorKind::TooLarge);
    assert!(message.contains("bytes"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_failed_baseline_scan_leaves_no_baseline_and_says_so() -> TestResult {
    let rig = rig(|f| {
        home_seed(f);
        f.broken = Some(BASELINE_PATH.into());
    })
    .await?;
    // The lease still came up; only the implicit baseline is missing.
    let (kind, message) = failed(&changes(&rig, None, None).await?)?;
    assert_eq!(kind, FileOpErrorKind::InvalidRequest);
    assert!(message.contains("checkpoint"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_failing_scan_surfaces_its_exit_and_stderr() -> TestResult {
    let rig = rig(|f| {
        home_seed(f);
        f.dir("/work");
        f.broken = Some("/work".into());
    })
    .await?;
    let (kind, message) = failed(&checkpoint(&rig, "/work", false).await?)?;
    assert_eq!(kind, FileOpErrorKind::Io);
    assert!(message.contains("exited 1") && message.contains("boom"), "{message}");
    Ok(())
}
