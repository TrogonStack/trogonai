//! A terminal driven over NATS end to end against an in-process fake envd: open, input
//! echo, resize, close and exit code, plus the SSH bridge's view of the same PTYs.
//!
//! The fake speaks the Connect framing and the field names of E2B's
//! `spec/envd/process/process.proto`. It is NOT a live E2B sandbox; nothing here proves
//! behaviour against the real service.

mod support;

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_nats::Client;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use futures::StreamExt;
use serde_json::{Value, json};
use support::{Boxed, TestResult};
use tokio::sync::mpsc;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, ProviderKind, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalInput,
    TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals, run_terminal_handler};
use trogon_sandbox_e2b_worker::{E2bApi, E2bConfig, E2bExec, E2bWorkerProvider, Envd};
use trogon_sandbox_protocol::{Provider, subjects, terminal_output_bytes};
use url::Url;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn frame(flags: u8, value: &Value) -> Result<Bytes, StatusCode> {
    let body = serde_json::to_vec(value).map_err(internal)?;
    let mut out = vec![flags];
    out.extend_from_slice(&u32::try_from(body.len()).map_err(internal)?.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(Bytes::from(out))
}

fn data_frame(text: &str) -> Result<Bytes, StatusCode> {
    frame(0, &json!({"event": {"data": {"pty": b64(text.as_bytes())}}}))
}

/// One fake PTY process.
struct Process {
    tag: String,
    pid: u32,
    out: mpsc::Sender<Bytes>,
    line: Vec<u8>,
}

#[derive(Default)]
struct Seen {
    starts: Vec<Value>,
    start_headers: Vec<(Option<String>, Option<String>)>,
    inputs: Vec<Value>,
    resizes: Vec<Value>,
    signals: Vec<Value>,
    deleted: usize,
}

#[derive(Default)]
struct Fake {
    processes: Vec<Process>,
    seen: Seen,
    next_pid: u32,
    /// Hold the start event back, to exercise the start timeout path.
    silent_start: bool,
    sandbox_alive: bool,
}

type Shared = Arc<Mutex<Fake>>;

fn selector_matches(p: &Process, selector: &Value) -> bool {
    selector.get("pid").and_then(Value::as_u64) == Some(u64::from(p.pid))
        || selector.get("tag").and_then(Value::as_str) == Some(p.tag.as_str())
}

async fn create_sandbox(State(fake): State<Shared>) -> impl IntoResponse {
    locked(&fake).sandbox_alive = true;
    (
        StatusCode::CREATED,
        axum::Json(json!({
            "sandboxID": "sbx1",
            "templateID": "base",
            "envdAccessToken": "envd-tok",
        })),
    )
}

async fn delete_sandbox(State(fake): State<Shared>, Path(id): Path<String>) -> StatusCode {
    let mut fake = locked(&fake);
    fake.seen.deleted += 1;
    fake.sandbox_alive = id != "sbx1";
    StatusCode::NO_CONTENT
}

async fn sandbox_detail(State(fake): State<Shared>) -> Result<Response, StatusCode> {
    let alive = locked(&fake).sandbox_alive;
    let state = alive.then(|| axum::Json(json!({"state": "running"})));
    Ok(state.ok_or(StatusCode::NOT_FOUND)?.into_response())
}

fn ended(value: Value) -> Result<Bytes, StatusCode> {
    Ok([frame(0, &json!({"event": {"end": value}}))?, frame(2, &json!({}))?]
        .concat()
        .into())
}

fn stream_response(body: Body) -> Result<Response, StatusCode> {
    Response::builder()
        .header("content-type", "application/connect+json")
        .body(body)
        .map_err(internal)
}

async fn start(State(fake): State<Shared>, headers: HeaderMap, body: Bytes) -> Result<Response, StatusCode> {
    let request: Value = serde_json::from_slice(body.get(5..).ok_or(StatusCode::BAD_REQUEST)?).map_err(internal)?;
    if request["process"].get("pty").is_none() && request.get("pty").is_none() {
        // Not a terminal: the worker's lease-start change-tracking scan. This fake runs no
        // commands, so the scan fails fast and the lease goes without a baseline.
        return stream_response(Body::from(ended(json!({"exited": true, "exitCode": 1}))?));
    }
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    let (pid, silent) = {
        let mut f = locked(&fake);
        f.next_pid += 1;
        let pid = 4000 + f.next_pid;
        f.seen.starts.push(request.clone());
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        f.seen
            .start_headers
            .push((header("x-access-token"), header("authorization")));
        let silent = f.silent_start;
        f.processes.push(Process {
            tag: request["tag"].as_str().unwrap_or_default().to_string(),
            pid,
            out: tx.clone(),
            line: Vec::new(),
        });
        (pid, silent)
    };
    if !silent {
        tx.send(frame(0, &json!({"event": {"start": {"pid": pid}}}))?)
            .await
            .map_err(internal)?;
        tx.send(data_frame("ready\r\n")?).await.map_err(internal)?;
    }
    // The process's own sender lives in `processes`; dropping it ends the stream.
    drop(tx);
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<_, Infallible>(b), rx))
    });
    stream_response(Body::from_stream(stream))
}

async fn send_input(State(fake): State<Shared>, axum::Json(req): axum::Json<Value>) -> Result<Response, StatusCode> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(req["input"]["pty"].as_str().ok_or(StatusCode::BAD_REQUEST)?)
        .map_err(internal)?;
    let mut replies: Vec<(mpsc::Sender<Bytes>, Bytes)> = Vec::new();
    let mut exited: Option<u32> = None;
    {
        let mut f = locked(&fake);
        f.seen.inputs.push(req.clone());
        let p = f
            .processes
            .iter_mut()
            .find(|p| selector_matches(p, &req["process"]))
            .ok_or(StatusCode::NOT_FOUND)?;
        p.line.extend_from_slice(&data);
        while let Some(end) = p.line.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = p.line.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if let Some(rest) = line.strip_prefix("echo ") {
                replies.push((p.out.clone(), data_frame(&format!("{rest}\r\n"))?));
            } else if line == "exit" {
                exited = Some(p.pid);
            }
        }
    }
    for (out, f) in replies {
        out.send(f).await.map_err(internal)?;
    }
    if let Some(pid) = exited {
        let out = {
            let mut f = locked(&fake);
            let idx = f
                .processes
                .iter()
                .position(|p| p.pid == pid)
                .ok_or(StatusCode::NOT_FOUND)?;
            f.processes.remove(idx).out
        };
        out.send(ended(
            json!({"exitCode": 3, "exited": true, "status": "exit status 3"}),
        )?)
        .await
        .map_err(internal)?;
    }
    Ok(axum::Json(json!({})).into_response())
}

async fn update(State(fake): State<Shared>, axum::Json(req): axum::Json<Value>) -> Result<Response, StatusCode> {
    let out = {
        let mut f = locked(&fake);
        f.seen.resizes.push(req.clone());
        f.processes
            .iter()
            .find(|p| selector_matches(p, &req["process"]))
            .map(|p| p.out.clone())
            .ok_or(StatusCode::NOT_FOUND)?
    };
    let size = &req["pty"]["size"];
    out.send(data_frame(&format!("size {}x{}\r\n", size["cols"], size["rows"]))?)
        .await
        .map_err(internal)?;
    Ok(axum::Json(json!({})).into_response())
}

async fn send_signal(State(fake): State<Shared>, axum::Json(req): axum::Json<Value>) -> Result<Response, StatusCode> {
    let out = {
        let mut f = locked(&fake);
        f.seen.signals.push(req.clone());
        let idx = f
            .processes
            .iter()
            .position(|p| selector_matches(p, &req["process"]))
            .ok_or(StatusCode::NOT_FOUND)?;
        f.processes.remove(idx).out
    };
    out.send(ended(json!({"exited": false, "status": "signal: killed"}))?)
        .await
        .map_err(internal)?;
    Ok(axum::Json(json!({})).into_response())
}

struct Rig {
    _nats: JetStreamTestServer,
    client: Client,
    fake: Shared,
    hub: Arc<E2bTerminals>,
    provider: Arc<E2bWorkerProvider>,
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

async fn rig() -> Result<Rig, Boxed> {
    rig_with_start_timeout(Duration::from_secs(10)).await
}

async fn rig_with_start_timeout(start_timeout: Duration) -> Result<Rig, Boxed> {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let fake: Shared = Shared::default();
    let app = Router::new()
        .route("/v2/sandboxes", post(create_sandbox))
        .route("/sandboxes/{id}", get(sandbox_detail).delete(delete_sandbox))
        .route("/process.Process/Start", post(start))
        .route("/process.Process/SendInput", post(send_input))
        .route("/process.Process/Update", post(update))
        .route("/process.Process/SendSignal", post(send_signal))
        .with_state(Arc::clone(&fake));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let config = E2bConfig::new(origin.clone(), "e2b.test", "api-key")?.with_envd_url(origin)?;
    let api = E2bApi::new(config, Duration::from_secs(5))?;
    let envd = Envd::new(Duration::from_secs(5))?;
    let exec = Arc::new(E2bExec::new(envd.clone()));
    let provider = Arc::new(E2bWorkerProvider::new(api, "base", exec));
    provider.lease(&lease_request()?, &LeaseId::new("lease-1")).await?;

    let shell = Arc::new(E2bGuestShell::new(Arc::clone(&provider), envd).with_start_timeout(start_timeout));
    let hub = Arc::new(E2bTerminals::new(shell, client.clone()));
    tokio::spawn(run_terminal_handler(client.clone(), Arc::clone(&hub)));
    Ok(Rig {
        _nats: nats,
        client,
        fake,
        hub,
        provider,
    })
}

fn request(lease: &str, id: &str, op: TerminalOp) -> Result<TerminalRequest, Boxed> {
    Ok(TerminalRequest {
        lease_id: LeaseId::new(lease),
        terminal_id: TerminalId::new(id)?,
        op,
    })
}

async fn send(rig: &Rig, lease: &str, id: &str, op: TerminalOp) -> Result<TerminalResponse, Boxed> {
    let req = request(lease, id, op)?;
    let subject = subjects::provider_terminal(ProviderKind::E2b, &req.lease_id);
    let body = serde_json::to_vec(&req)?;
    // The handler subscribes in a spawned task; retry until it is listening.
    let msg = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let reply = tokio::time::timeout(
                Duration::from_millis(500),
                rig.client.request(subject.clone(), body.clone().into()),
            )
            .await;
            if let Ok(Ok(msg)) = reply {
                break msg;
            }
        }
    })
    .await?;
    Ok(serde_json::from_slice(&msg.payload)?)
}

fn open_op(cols: u16, rows: u16) -> Result<TerminalOp, Boxed> {
    Ok(TerminalOp::Open {
        cols: TerminalDimension::new(cols)?,
        rows: TerminalDimension::new(rows)?,
        cmd: vec![],
        env: BTreeMap::new(),
        cwd: None,
        resume_from: None,
    })
}

fn input_op(text: &str) -> Result<TerminalOp, Boxed> {
    Ok(TerminalOp::Input {
        data: TerminalInput::new(b64(text.as_bytes()))?,
    })
}

async fn read_until(sub: &mut async_nats::Subscriber, want: &str) -> Result<String, Boxed> {
    let mut seen = String::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen.contains(want) {
            let msg = sub.next().await.ok_or("the subscription closed")?;
            let frame: TerminalOutput = serde_json::from_slice(&msg.payload)?;
            let bytes = terminal_output_bytes(&frame).ok_or("the terminal ended early")?;
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        Ok::<(), Boxed>(())
    })
    .await??;
    Ok(seen)
}

async fn next_exit(sub: &mut async_nats::Subscriber) -> Result<Option<i32>, Boxed> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let msg = sub.next().await.ok_or("the subscription closed")?;
            if let TerminalOutput::Exit { exit_code, .. } = serde_json::from_slice(&msg.payload)? {
                return Ok::<_, Boxed>(exit_code);
            }
        }
    })
    .await?
}

async fn wait_for(mut ready: impl FnMut() -> bool) -> TestResult {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if ready() {
                break;
            }
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_terminal_opens_types_resizes_and_closes_through_nats() -> TestResult {
    let rig = rig().await?;
    let id = TerminalId::new("term-1")?;
    let mut sub = rig.client.subscribe(subjects::terminal_output(&id)).await?;

    let opened = send(&rig, "lease-1", "term-1", open_op(100, 30)?).await?;
    assert_eq!(opened, TerminalResponse::Opened { terminal_id: id });
    read_until(&mut sub, "ready").await?;

    assert_eq!(
        send(&rig, "lease-1", "term-1", input_op("echo hello\n")?).await?,
        TerminalResponse::Done
    );
    read_until(&mut sub, "hello").await?;

    let resize = TerminalOp::Resize {
        cols: TerminalDimension::new(120)?,
        rows: TerminalDimension::new(40)?,
    };
    assert_eq!(send(&rig, "lease-1", "term-1", resize).await?, TerminalResponse::Done);
    read_until(&mut sub, "size 120x40").await?;

    assert_eq!(
        send(&rig, "lease-1", "term-1", TerminalOp::Close).await?,
        TerminalResponse::Done
    );
    assert_eq!(next_exit(&mut sub).await?, None);
    wait_for(|| !locked(&rig.fake).seen.signals.is_empty()).await?;

    let f = locked(&rig.fake);
    let start = &f.seen.starts[0];
    assert_eq!(start["pty"]["size"], json!({"cols": 100, "rows": 30}));
    assert_eq!(start["process"]["cmd"], "/bin/bash");
    assert_eq!(start["process"]["args"], json!(["-l"]));
    assert_eq!(start["process"]["envs"]["TERM"], "xterm-256color");
    let tag = start["tag"].as_str().ok_or("the start carries a tag")?;
    assert!(tag.starts_with("trogon-term-"), "{tag}");
    // The sandbox's own token authenticates the call, never the API key.
    assert_eq!(f.seen.start_headers[0].0.as_deref(), Some("envd-tok"));
    assert!(
        !format!("{:?}", f.seen.start_headers).contains("api-key"),
        "the API key must not reach envd"
    );
    // Once the pid is known every call addresses the process by pid.
    assert_eq!(f.seen.inputs[0]["process"], json!({"pid": 4001}));
    assert_eq!(f.seen.inputs[0]["input"]["pty"], b64(b"echo hello\n"));
    assert_eq!(f.seen.resizes[0]["process"], json!({"pid": 4001}));
    assert_eq!(f.seen.resizes[0]["pty"]["size"], json!({"cols": 120, "rows": 40}));
    assert_eq!(f.seen.signals[0]["process"], json!({"pid": 4001}));
    assert_eq!(f.seen.signals[0]["signal"], "SIGNAL_SIGKILL");
    Ok(())
}

#[tokio::test]
async fn a_command_runs_in_a_login_shell_and_its_exit_code_is_reported() -> TestResult {
    let rig = rig().await?;
    let id = TerminalId::new("term-x")?;
    let mut sub = rig.client.subscribe(subjects::terminal_output(&id)).await?;
    let op = TerminalOp::Open {
        cols: TerminalDimension::new(80)?,
        rows: TerminalDimension::new(24)?,
        cmd: vec![TerminalArgument::new("top")?],
        env: BTreeMap::from([("TERM".to_string(), "vt100".to_string())]),
        cwd: Some("/w".to_string()),
        resume_from: None,
    };
    let opened = send(&rig, "lease-1", "term-x", op).await?;
    assert_eq!(opened, TerminalResponse::Opened { terminal_id: id });
    {
        let f = locked(&rig.fake);
        let start = &f.seen.starts[0];
        assert_eq!(
            start["process"]["args"],
            json!(["-l", "-c", "cd '/w' || exit 1; export TERM='vt100'; exec 'top'"])
        );
        assert_eq!(start["process"]["envs"]["TERM"], "vt100");
    }

    send(&rig, "lease-1", "term-x", input_op("exit\n")?).await?;
    assert_eq!(next_exit(&mut sub).await?, Some(3));
    assert!(
        locked(&rig.fake).seen.signals.is_empty(),
        "a process that exited on its own is not killed"
    );
    Ok(())
}

#[tokio::test]
async fn a_start_that_never_reports_a_pid_fails_the_open_and_kills_by_tag() -> TestResult {
    let rig = rig_with_start_timeout(Duration::from_millis(100)).await?;
    locked(&rig.fake).silent_start = true;
    let reply = send(&rig, "lease-1", "silent", open_op(80, 24)?).await?;
    assert!(
        matches!(reply, TerminalResponse::Failed(TerminalError::SpawnFailed { .. })),
        "{reply:?}"
    );
    let f = locked(&rig.fake);
    let tag = f.seen.starts[0]["tag"].as_str().ok_or("the start carries a tag")?;
    assert_eq!(f.seen.signals[0]["process"], json!({ "tag": tag }));
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_ends_its_terminals() -> TestResult {
    let rig = rig().await?;
    let id = TerminalId::new("t")?;
    let mut sub = rig.client.subscribe(subjects::terminal_output(&id)).await?;
    send(&rig, "lease-1", "t", open_op(80, 24)?).await?;
    read_until(&mut sub, "ready").await?;

    rig.hub.close_lease(&LeaseId::new("lease-1")).await;
    assert_eq!(next_exit(&mut sub).await?, None);
    wait_for(|| !locked(&rig.fake).seen.signals.is_empty()).await?;
    rig.provider.cancel(&LeaseId::new("lease-1")).await?;
    assert_eq!(locked(&rig.fake).seen.deleted, 1);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let rig = rig().await?;
    // Make sure the handler is subscribed by round-tripping a held lease.
    send(&rig, "lease-1", "warm", open_op(80, 24)?).await?;
    let req = request("lease-other", "t", open_op(80, 24)?)?;
    let reply = tokio::time::timeout(
        Duration::from_millis(500),
        rig.client.request(
            subjects::provider_terminal(ProviderKind::E2b, &req.lease_id),
            serde_json::to_vec(&req)?.into(),
        ),
    )
    .await;
    assert!(!matches!(reply, Ok(Ok(_))), "an unheld lease must not be answered");
    Ok(())
}
