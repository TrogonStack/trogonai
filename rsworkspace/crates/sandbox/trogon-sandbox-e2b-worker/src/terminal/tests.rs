use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    LeaseId, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, Port, ProviderKind,
    SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, TerminalDimension, WorkspaceSource,
};
use trogon_sandbox_protocol::Provider;
use url::Url;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::constants::TERMINAL_SUBJECT;
use super::pty_process::PtyProcess;
use super::pump_input::pump_input;
use super::pump_output::pump_output;
use super::pump_resize::pump_resize;
use super::{E2bGuestShell, E2bTerminals, GuestError, GuestShell, TerminalError, run_terminal_handler};
use crate::adapter::E2bWorkerProvider;
use crate::api::{E2bApi, E2bConfig};
use crate::envd::{Envd, EnvdTarget, ProcessSelector, PtyRequest};
use crate::exec::E2bExec;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TIMEOUT: Duration = Duration::from_secs(5);
const END_STREAM: u8 = 0b10;

fn frame(flags: u8, value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let body = serde_json::to_vec(value)?;
    let mut out = vec![flags];
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// A Connect stream of `events` closed by `trailer`.
fn stream_of(events: &[Value], trailer: Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    for event in events {
        out.extend(frame(0, event)?);
    }
    out.extend(frame(END_STREAM, &trailer)?);
    Ok(out)
}

fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

fn terminal_cause(error: GuestError) -> Result<TerminalError, Box<dyn std::error::Error>> {
    match error {
        GuestError::Unavailable(cause) => cause
            .downcast::<TerminalError>()
            .map(|cause| *cause)
            .map_err(|_| "the cause is not a terminal error".into()),
        other => Err(format!("{other:?}").into()),
    }
}

fn dimension(value: u16) -> Result<TerminalDimension, Box<dyn std::error::Error>> {
    Ok(TerminalDimension::new(value)?)
}

fn start_body(request: &Request) -> Option<Value> {
    serde_json::from_slice(request.body.get(5..)?).ok()
}

fn unary_body(request: &Request) -> Option<Value> {
    serde_json::from_slice(&request.body).ok()
}

fn spec_parts(image: SandboxImage) -> Result<SandboxSpecParts, Box<dyn std::error::Error>> {
    Ok(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: std::num::NonZeroU32::MIN.saturating_add(3599),
        memory_mib: std::num::NonZeroU32::MIN.saturating_add(511),
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: vec![Port::new(3000)?],
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })
}

fn lease_request() -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let image = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified("my-template"),
        pull_secret: None,
    };
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("t")?,
        provider_pref: vec![ProviderKind::E2b],
        spec: SandboxSpec::new(spec_parts(image)?)?,
        ttl_seconds: 600,
        idempotency_key: LeaseRequestKey::new("k")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: vec![],
    })
}

fn config(api: &MockServer, envd_url: &str) -> Result<E2bConfig, Box<dyn std::error::Error>> {
    Ok(E2bConfig::new(Url::parse(&api.uri())?, "e2b.test", "k")?.with_envd_url(Url::parse(envd_url)?)?)
}

/// A shell over a provider that holds lease `l1`, with envd served at `envd_url`.
async fn held_shell(api: &MockServer, envd_url: &str) -> Result<E2bGuestShell, Box<dyn std::error::Error>> {
    Mock::given(method("POST"))
        .and(path("/v2/sandboxes"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "sandboxID": "sbx1",
            "templateID": "my-template",
            "envdAccessToken": "envd-tok"
        })))
        .mount(api)
        .await;
    let provider = E2bWorkerProvider::new(
        E2bApi::new(config(api, envd_url)?, TIMEOUT)?,
        "base",
        Arc::new(E2bExec::new(Envd::new(TIMEOUT)?)),
    );
    provider.lease(&lease_request()?, &LeaseId::new("l1")).await?;
    Ok(E2bGuestShell::new(Arc::new(provider), Envd::new(TIMEOUT)?))
}

async fn mount_start(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(response)
        .mount(server)
        .await;
}

fn pty_request() -> PtyRequest {
    PtyRequest {
        tag: "t1".to_owned(),
        cmd: "/bin/bash".to_owned(),
        args: vec!["-l".to_owned()],
        env: Default::default(),
        user: "user".to_owned(),
        cols: 80,
        rows: 24,
    }
}

fn process_for(server: &MockServer) -> Result<PtyProcess, Box<dyn std::error::Error>> {
    let config = config(server, &server.uri())?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, Some("tok".to_owned()))?;
    Ok(PtyProcess {
        envd: Envd::new(TIMEOUT)?,
        target,
        selector: ProcessSelector::Pid(7),
    })
}

/// An envd whose `Start` stream stays open without ever reporting the process started.
async fn stalled_envd() -> Result<String, Box<dyn std::error::Error>> {
    let keepalive = Bytes::from(frame(0, &json!({"event": {"keepalive": {}}}))?);
    let app = Router::new()
        .route(
            "/process.Process/Start",
            post(move || {
                let keepalive = keepalive.clone();
                async move {
                    let open = futures::stream::once(async move { Ok::<_, Infallible>(keepalive) })
                        .chain(futures::stream::pending());
                    Body::from_stream(open)
                }
            }),
        )
        .route("/process.Process/SendSignal", post(|| async { "{}" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(url)
}

fn unavailable(result: Result<i32, GuestError>) -> Result<String, Box<dyn std::error::Error>> {
    match result {
        Err(GuestError::Unavailable(source)) => Ok(source.to_string()),
        other => Err(format!("expected an unavailable guest, got {other:?}").into()),
    }
}

#[test]
fn the_broadcast_subject_names_the_provider() {
    assert_eq!(TERMINAL_SUBJECT, "sandbox.provider.e2b.terminal.>");
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_is_refused() -> TestResult {
    let api = MockServer::start().await;
    let provider = E2bWorkerProvider::new(
        E2bApi::new(config(&api, &api.uri())?, TIMEOUT)?,
        "base",
        Arc::new(E2bExec::new(Envd::new(TIMEOUT)?)),
    );
    let shell = E2bGuestShell::new(Arc::new(provider), Envd::new(TIMEOUT)?);
    let lease = LeaseId::new("none");
    assert!(!shell.holds(&lease).await);
    let refused = shell
        .open(&lease, (dimension(80)?, dimension(24)?), "xterm", None)
        .await
        .err()
        .ok_or("open succeeded")?;
    assert_eq!(
        refused.to_string(),
        format!("sandbox unavailable: {}", TerminalError::NotHeld)
    );
    assert_eq!(
        shell.not_held_reason(),
        "the worker holds no E2B sandbox for this lease"
    );
    shell.release(&lease).await;
    Ok(())
}

#[tokio::test]
async fn a_login_shell_streams_the_pty_until_it_exits() -> TestResult {
    let server = MockServer::start().await;
    let shell = held_shell(&server, &server.uri()).await?;
    let body = stream_of(
        &[
            json!({"event": {"start": {"pid": 42}}}),
            json!({"event": {"data": {"pty": b64("$ ")}}}),
            json!({"event": {"end": {"exited": true, "exitCode": 3}}}),
        ],
        json!({}),
    )?;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(|request: &Request| {
            start_body(request).is_some_and(|body| {
                body["tag"].as_str().is_some_and(|tag| tag.starts_with("trogon-term-"))
                    && body["pty"] == json!({"size": {"cols": 100, "rows": 30}})
                    && body["process"]["cmd"] == "/bin/bash"
                    && body["process"]["args"] == json!(["-l"])
                    && body["process"]["envs"] == json!({"TERM": "xterm-256color"})
            })
        })
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;
    assert!(shell.holds(&LeaseId::new("l1")).await);
    let mut channel = shell
        .open(
            &LeaseId::new("l1"),
            (dimension(100)?, dimension(30)?),
            "xterm-256color",
            None,
        )
        .await?;
    let mut output = Vec::new();
    channel.stdout.read_to_end(&mut output).await?;
    assert_eq!(output, b"$ ");
    assert_eq!(channel.completion.await?, 3);
    Ok(())
}

#[tokio::test]
async fn a_command_runs_under_the_login_shell() -> TestResult {
    let server = MockServer::start().await;
    let shell = held_shell(&server, &server.uri()).await?;
    let body = stream_of(
        &[
            json!({"event": {"start": {"pid": 1}}}),
            json!({"event": {"end": {"exited": true, "exitCode": 0}}}),
        ],
        json!({}),
    )?;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(|request: &Request| {
            start_body(request).is_some_and(|body| body["process"]["args"] == json!(["-l", "-c", "cd /w && ls"]))
        })
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .expect(1)
        .mount(&server)
        .await;
    let channel = shell
        .open(
            &LeaseId::new("l1"),
            (dimension(80)?, dimension(24)?),
            "xterm",
            Some("cd /w && ls".to_owned()),
        )
        .await?;
    assert_eq!(channel.completion.await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_process_that_never_reports_a_start_is_killed_by_its_tag() -> TestResult {
    let server = MockServer::start().await;
    let shell = held_shell(&server, &server.uri()).await?;
    mount_start(
        &server,
        ResponseTemplate::new(200).set_body_bytes(stream_of(
            &[json!({"event": {"data": {"pty": b64("early")}}})],
            json!({}),
        )?),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/process.Process/SendSignal"))
        .and(|request: &Request| {
            unary_body(request).is_some_and(|body| {
                body["process"]["tag"]
                    .as_str()
                    .is_some_and(|tag| tag.starts_with("trogon-term-"))
            })
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let refused = shell
        .open(&LeaseId::new("l1"), (dimension(80)?, dimension(24)?), "xterm", None)
        .await
        .err()
        .ok_or("open succeeded")?;
    assert!(matches!(terminal_cause(refused)?, TerminalError::NotStarted));
    Ok(())
}

#[tokio::test]
async fn an_envd_failure_before_the_start_is_reported() -> TestResult {
    let server = MockServer::start().await;
    let shell = held_shell(&server, &server.uri()).await?;
    mount_start(
        &server,
        ResponseTemplate::new(200).set_body_bytes(stream_of(
            &[],
            json!({"error": {"code": "permission_denied", "message": "no"}}),
        )?),
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/process.Process/SendSignal"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let refused = shell
        .open(&LeaseId::new("l1"), (dimension(80)?, dimension(24)?), "xterm", None)
        .await
        .err()
        .ok_or("open succeeded")?;
    assert!(matches!(terminal_cause(refused)?, TerminalError::Envd(_)));
    Ok(())
}

#[tokio::test]
async fn an_envd_that_refuses_the_start_call_is_reported() -> TestResult {
    let server = MockServer::start().await;
    let shell = held_shell(&server, &server.uri()).await?;
    mount_start(
        &server,
        ResponseTemplate::new(401).set_body_json(json!({"message": "bad"})),
    )
    .await;
    let refused = shell
        .open(&LeaseId::new("l1"), (dimension(80)?, dimension(24)?), "xterm", None)
        .await
        .err()
        .ok_or("open succeeded")?;
    assert!(matches!(terminal_cause(refused)?, TerminalError::Envd(_)));
    Ok(())
}

#[tokio::test]
async fn a_start_that_takes_too_long_times_out() -> TestResult {
    let api = MockServer::start().await;
    let envd_url = stalled_envd().await?;
    let shell = held_shell(&api, &envd_url)
        .await?
        .with_start_timeout(Duration::from_millis(100));
    let refused = shell
        .open(&LeaseId::new("l1"), (dimension(80)?, dimension(24)?), "xterm", None)
        .await
        .err()
        .ok_or("open succeeded")?;
    assert!(matches!(terminal_cause(refused)?, TerminalError::StartTimeout));
    Ok(())
}

#[tokio::test]
async fn input_reaches_the_pty_in_order_and_stops_when_the_hub_hangs_up() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/SendInput"))
        .and(body_json(json!({"process": {"pid": 7}, "input": {"pty": b64("ls\n")}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let (mut hub, pipe) = tokio::io::duplex(64);
    let pump = tokio::spawn(pump_input(process_for(&server)?, pipe, CancellationToken::new()));
    hub.write_all(b"ls\n").await?;
    drop(hub);
    pump.await?;
    Ok(())
}

#[tokio::test]
async fn refused_input_ends_the_input_pump() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/SendInput"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let (mut hub, pipe) = tokio::io::duplex(64);
    let pump = tokio::spawn(pump_input(process_for(&server)?, pipe, CancellationToken::new()));
    hub.write_all(b"x").await?;
    pump.await?;
    Ok(())
}

#[tokio::test]
async fn a_finished_terminal_retires_the_input_pump() -> TestResult {
    let server = MockServer::start().await;
    let (_hub, pipe) = tokio::io::duplex(64);
    let done = CancellationToken::new();
    done.cancel();
    pump_input(process_for(&server)?, pipe, done).await;
    Ok(())
}

#[tokio::test]
async fn resizes_reach_the_pty_and_stop_when_the_hub_hangs_up() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Update"))
        .and(body_json(
            json!({"process": {"pid": 7}, "pty": {"size": {"cols": 100, "rows": 30}}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let (resize, sizes) = mpsc::channel(2);
    let pump = tokio::spawn(pump_resize(process_for(&server)?, sizes, CancellationToken::new()));
    resize.send((dimension(100)?, dimension(30)?)).await?;
    drop(resize);
    pump.await?;
    Ok(())
}

#[tokio::test]
async fn a_refused_resize_does_not_end_the_resize_pump() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Update"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;
    let (resize, sizes) = mpsc::channel(2);
    let pump = tokio::spawn(pump_resize(process_for(&server)?, sizes, CancellationToken::new()));
    resize.send((dimension(100)?, dimension(30)?)).await?;
    resize.send((dimension(90)?, dimension(20)?)).await?;
    drop(resize);
    pump.await?;
    Ok(())
}

#[tokio::test]
async fn a_finished_terminal_retires_the_resize_pump() -> TestResult {
    let server = MockServer::start().await;
    let (_resize, sizes) = mpsc::channel(1);
    let done = CancellationToken::new();
    done.cancel();
    pump_resize(process_for(&server)?, sizes, done).await;
    Ok(())
}

/// Runs `pump_output` over a stream of `body`, returning what it reports and whether the terminal was retired.
async fn run_output(
    server: &MockServer,
    body: Vec<u8>,
    close: CancellationToken,
    read_end: bool,
) -> Result<(Result<i32, GuestError>, bool), Box<dyn std::error::Error>> {
    mount_start(server, ResponseTemplate::new(200).set_body_bytes(body)).await;
    let process = process_for(server)?;
    let stream = process.envd.start_pty(&process.target, &pty_request()).await?;
    let (out, read) = tokio::io::duplex(1);
    let _read = read_end.then_some(read);
    let (exit_tx, exit_rx) = oneshot::channel();
    let done = CancellationToken::new();
    tokio::spawn(pump_output(process, stream, out, close, done.clone(), exit_tx));
    let result = exit_rx.await?;
    Ok((result, done.is_cancelled()))
}

#[tokio::test]
async fn closing_the_terminal_kills_the_process() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/SendSignal"))
        .and(body_json(json!({"process": {"pid": 7}, "signal": "SIGNAL_SIGKILL"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let close = CancellationToken::new();
    close.cancel();
    let body = stream_of(&[json!({"event": {"start": {"pid": 7}}})], json!({}))?;
    let (result, retired) = run_output(&server, body, close, true).await?;
    assert_eq!(unavailable(result)?, "the terminal was closed");
    assert!(retired);
    Ok(())
}

#[tokio::test]
async fn a_stream_that_ends_without_an_exit_status_is_reported() -> TestResult {
    let server = MockServer::start().await;
    let body = stream_of(&[json!({"event": {"start": {"pid": 7}}})], json!({}))?;
    let (result, _) = run_output(&server, body, CancellationToken::new(), true).await?;
    assert_eq!(unavailable(result)?, "the envd stream ended without an exit status");
    Ok(())
}

#[tokio::test]
async fn a_dropped_output_pipe_fails_the_terminal() -> TestResult {
    let server = MockServer::start().await;
    let body = stream_of(&[json!({"event": {"data": {"pty": b64("lost")}}})], json!({}))?;
    let (result, _) = run_output(&server, body, CancellationToken::new(), false).await?;
    assert!(unavailable(result)?.contains("output pipe failed"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_terminal_handler_serves_until_its_connection_closes() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let api = MockServer::start().await;
    let shell = held_shell(&api, "http://127.0.0.1:1").await?;
    let terminals = Arc::new(E2bTerminals::new(Arc::new(shell), client.clone()));
    let handler = tokio::spawn(run_terminal_handler(client.clone(), terminals));
    tokio::time::sleep(Duration::from_millis(300)).await;
    client.drain().await?;
    handler.await??;
    Ok(())
}
