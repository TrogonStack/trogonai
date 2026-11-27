mod command_scope_tests;
mod control_tests;
mod controlled_tests;
mod custody_tests;
mod deadline_tests;
mod files_tests;
mod native_deadline_tests;
mod run_command_tests;
mod scripted_store_tests;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CommandExecution, ExecutionId, KillCommandRequest, KillCommandResponse, LaunchCancellation, LaunchKey, LeaseError,
    LeaseId, OutputTruncation, PollCommandResponse, ReadFileRequest, ReadFileResponse, RunCommandRequest,
    RunCommandResponse, SandboxFileWrite, SandboxUserName, StdinMode, StreamedOutput, UnixSignal, WriteFilesRequest,
    WriteFilesResponse, WriteStdinResponse,
};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

use super::clock::Clock;
use super::command_custody::CommandCustody;
use super::command_owner::CommandOwner;
use super::command_owner_error::CommandOwnerError;
use super::command_record::CommandRecord;
use super::command_record_error::CommandRecordError;
use super::command_store::CommandStore;
use super::constants::{
    EXECUTION_DIRECTORY_PREFIX, LAUNCH_SCRIPT, PROCESS_IDENTITY, REMOVE_DIRECTORY_SCRIPT, TRAFFIC_TOKEN_HEADER,
};
use super::e2b_exec::E2bExec;
use super::exec_error::ExecError;
use super::execution::Execution;
use super::failure::{after_launch, custody_unavailable, denied, invalid_spec};
use super::launch_fingerprint::LaunchFingerprint;
use super::run_command_violation::run_command_violation;
use crate::LeaseSandbox;
use crate::api::{ApiError, E2bConfig};
use crate::envd::{Envd, EnvdTarget};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const END_STREAM_FLAG: u8 = 0x02;

fn frame(flag: u8, body: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let body = serde_json::to_vec(body)?;
    let mut out = vec![flag];
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn stream(events: &[Value], end: Option<Value>) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    for event in events {
        out.extend(frame(0, event)?);
    }
    out.extend(frame(END_STREAM_FLAG, &end.unwrap_or_else(|| json!({})))?);
    Ok(out)
}

fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

fn pid_and_exit() -> Result<Vec<u8>, serde_json::Error> {
    stream(
        &[
            json!({"event": {"data": {"stdout": b64("123 456\n")}}}),
            json!({"event": {"end": {"exited": true}}}),
        ],
        None,
    )
}

fn exited(code: i64) -> Result<Vec<u8>, serde_json::Error> {
    stream(&[json!({"event": {"end": {"exited": true, "exitCode": code}}})], None)
}

/// Matches a `process.Process/Start` whose bash script is `script`.
struct Script(&'static str);

impl Match for Script {
    fn matches(&self, request: &Request) -> bool {
        request
            .body
            .get(5..)
            .and_then(|body| serde_json::from_slice::<Value>(body).ok())
            .is_some_and(|body| body["process"]["args"][6] == self.0)
    }
}

/// Matches a `GET /files` whose path ends with `suffix`.
struct FileEnding(&'static str);

impl Match for FileEnding {
    fn matches(&self, request: &Request) -> bool {
        request
            .url
            .query_pairs()
            .any(|(key, value)| key == "path" && value.ends_with(self.0))
    }
}

fn config(server: &MockServer) -> Result<E2bConfig, Box<dyn std::error::Error>> {
    Ok(
        E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "k")?
            .with_envd_url(Url::parse(&server.uri())?)?,
    )
}

fn fixture(server: &MockServer) -> Result<(E2bExec, LeaseSandbox), Box<dyn std::error::Error>> {
    let target = EnvdTarget::for_sandbox(&config(server)?, "sbx1", None, Some("tok".to_owned()))?;
    let sandbox = LeaseSandbox {
        sandbox_id: "sbx1".to_owned(),
        target,
        traffic_token: None,
        domain: "e2b.app".to_owned(),
        exposed_ports: vec![],
    };
    Ok((E2bExec::new(Envd::new(Duration::from_secs(5))?), sandbox))
}

fn run_request(command: &[&str]) -> RunCommandRequest {
    RunCommandRequest {
        lease_id: LeaseId::new("lease-1"),
        command: command.iter().map(|part| (*part).to_owned()).collect(),
        cwd: None,
        env: Default::default(),
        timeout_seconds: 60,
        stream_output: StreamedOutput::default(),
        run_as_user: None,
        output_retention: Default::default(),
        max_output_bytes: None,
        redact_values: Default::default(),
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::default(),
        execution: CommandExecution::Attached,
    }
}

fn detached(command: &[&str]) -> RunCommandRequest {
    RunCommandRequest {
        execution: CommandExecution::Detached {
            stdin: StdinMode::Closed,
            deadline: Default::default(),
            process: None,
        },
        ..run_request(command)
    }
}

async fn mount_script(server: &MockServer, script: &'static str, body: Vec<u8>) {
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(Script(script))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(server)
        .await;
}

async fn mount_file(server: &MockServer, suffix: &'static str, body: Option<&str>) {
    let reply = match body {
        Some(body) => ResponseTemplate::new(200).set_body_string(body.to_owned()),
        None => ResponseTemplate::new(404).set_body_json(json!({"code": 404, "message": "not found"})),
    };
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(FileEnding(suffix))
        .respond_with(reply)
        .mount(server)
        .await;
}

async fn mount_completed_foreground(
    server: &MockServer,
    stdout: &str,
    stderr: &str,
    code: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    mount_file(server, "/exit", Some(&format!("{code}\n"))).await;
    mount_file(server, "/out", Some(stdout)).await;
    mount_file(server, "/err", Some(stderr)).await;
    mount_script(server, PROCESS_IDENTITY, exited(44)?).await;
    mount_script(server, REMOVE_DIRECTORY_SCRIPT, exited(0)?).await;
    mount_script(server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    Ok(())
}

fn backend(response: &LeaseError) -> bool {
    matches!(response, LeaseError::Backend { .. })
}

async fn started(exec: &E2bExec, sandbox: &LeaseSandbox, request: &RunCommandRequest) -> Result<ExecutionId, String> {
    match exec.handle_run_command(sandbox, request).await {
        RunCommandResponse::Started(started) => Ok(started.execution_id),
        other => Err(format!("unexpected {other:?}")),
    }
}

#[tokio::test]
async fn a_foreground_command_returns_output_and_exit_code() -> TestResult {
    let server = MockServer::start().await;
    mount_completed_foreground(&server, "out", "err", 2).await?;
    let (exec, sandbox) = fixture(&server)?;
    let RunCommandResponse::Completed(output) = exec.handle_run_command(&sandbox, &run_request(&["false"])).await
    else {
        return Err("the command must complete".into());
    };
    assert_eq!(output.exit_code, 2);
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
    assert!(!output.timed_out.is_timed_out());
    Ok(())
}

#[tokio::test]
async fn foreground_forwards_environment_cwd_user_and_timeout() -> TestResult {
    let server = MockServer::start().await;
    mount_completed_foreground(&server, "finished", "", 23).await?;
    let (exec, sandbox) = fixture(&server)?;
    let mut request = run_request(&["cat"]);
    request.cwd = Some("work".to_owned());
    request.env = std::iter::once(("CAP40_ENV".to_owned(), "literal space ' quote $HOME".to_owned()))
        .collect::<std::collections::BTreeMap<_, _>>()
        .try_into()?;
    request.run_as_user = Some(SandboxUserName::new("root")?);
    request.timeout_seconds = 17;
    let RunCommandResponse::Completed(output) = exec.handle_run_command(&sandbox, &request).await else {
        return Err("a valid foreground command must complete".into());
    };
    assert_eq!(output.stdout, b"finished");
    assert_eq!(output.exit_code, 23);
    let requests = server.received_requests().await.ok_or("requests are not recorded")?;
    let launch = requests
        .iter()
        .find(|request| Script(LAUNCH_SCRIPT).matches(request))
        .ok_or("the launch script was not started")?;
    let body: Value = serde_json::from_slice(&launch.body[5..])?;
    assert_eq!(body["process"]["cwd"], "/root/work");
    assert_eq!(
        body["process"]["envs"],
        json!({"CAP40_ENV": "literal space ' quote $HOME"})
    );
    assert_eq!(body["stdin"], false);
    assert_eq!(
        launch
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Basic cm9vdDo=")
    );
    assert_eq!(
        launch
            .headers
            .get("connect-timeout-ms")
            .and_then(|value| value.to_str().ok()),
        Some("30000")
    );
    Ok(())
}

#[tokio::test]
async fn a_relative_cwd_resolves_against_the_users_home() {
    let mut request = run_request(&["pwd"]);
    assert_eq!(E2bExec::cwd_of(&request), None);
    request.cwd = Some("sub".to_owned());
    assert_eq!(E2bExec::cwd_of(&request).as_deref(), Some("/home/user/sub"));
    request.cwd = Some("/abs".to_owned());
    assert_eq!(E2bExec::cwd_of(&request).as_deref(), Some("/abs"));
}

#[tokio::test]
async fn a_deadline_without_end_is_explicit_uncertainty() -> TestResult {
    let server = MockServer::start().await;
    let body = stream(
        &[],
        Some(json!({"error": {"code": "deadline_exceeded", "message": "slow"}})),
    )?;
    mount_script(&server, LAUNCH_SCRIPT, body).await;
    let (exec, sandbox) = fixture(&server)?;
    let RunCommandResponse::Denied(error) = exec.handle_run_command(&sandbox, &run_request(&["sleep", "9"])).await
    else {
        return Err("an unproven command must be denied".into());
    };
    assert!(backend(&error));
    Ok(())
}

#[tokio::test]
async fn a_native_helper_end_after_deadline_is_not_owned_command_completion() -> TestResult {
    let server = MockServer::start().await;
    let body = stream(
        &[json!({"event": {"end": {"exited": true, "exitCode": 137}}})],
        Some(json!({"error": {"code": "deadline_exceeded", "message": "slow"}})),
    )?;
    mount_script(&server, LAUNCH_SCRIPT, body).await;
    let (exec, sandbox) = fixture(&server)?;
    let RunCommandResponse::Denied(error) = exec.handle_run_command(&sandbox, &run_request(&["sleep", "9"])).await
    else {
        return Err("a helper end after the deadline must be denied".into());
    };
    assert!(backend(&error));
    Ok(())
}

#[tokio::test]
async fn output_is_capped_and_marked_truncated() -> TestResult {
    let server = MockServer::start().await;
    mount_completed_foreground(&server, &"x".repeat(1000), "", 0).await?;
    let (exec, sandbox) = fixture(&server)?;
    let mut request = run_request(&["x"]);
    request.max_output_bytes = Some(16);
    let RunCommandResponse::Completed(output) = exec.handle_run_command(&sandbox, &request).await else {
        return Err("the command must complete".into());
    };
    assert_eq!(output.truncated, OutputTruncation::Truncated);
    assert!(output.stdout.len() < 1000);
    Ok(())
}

#[tokio::test]
async fn unsupported_options_are_refused_with_a_reason() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = fixture(&server)?;
    let mut streamed = run_request(&["x"]);
    streamed.stream_output = StreamedOutput::Enabled;
    let interactive = RunCommandRequest {
        execution: CommandExecution::Detached {
            stdin: StdinMode::Interactive,
            deadline: Default::default(),
            process: None,
        },
        ..run_request(&["x"])
    };
    let named = RunCommandRequest {
        execution: CommandExecution::Detached {
            stdin: StdinMode::Closed,
            deadline: Default::default(),
            process: Some(Default::default()),
        },
        ..run_request(&["x"])
    };
    let mut cancellable = run_request(&["x"]);
    cancellable.require_launch_cancellation = LaunchCancellation::Required;
    for request in [run_request(&[]), streamed, interactive, named, cancellable] {
        assert!(matches!(
            exec.handle_run_command(&sandbox, &request).await,
            RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })
        ));
    }
    assert_eq!(run_command_violation(&run_request(&["x"])), None);
    Ok(())
}

#[tokio::test]
async fn a_backend_failure_is_denied_not_dropped() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": "unauthenticated", "message": "bad"})))
        .mount(&server)
        .await;
    let (exec, sandbox) = fixture(&server)?;
    assert!(matches!(
        exec.handle_run_command(&sandbox, &run_request(&["x"])).await,
        RunCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    assert!(matches!(
        exec.handle_run_command(&sandbox, &detached(&["x"])).await,
        RunCommandResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_detached_command_starts_then_polls_running_then_completed() -> TestResult {
    let server = MockServer::start().await;
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    let (exec, sandbox) = fixture(&server)?;
    let id = started(&exec, &sandbox, &detached(&["sleep", "5"])).await?;

    mount_file(&server, "/exit", None).await;
    assert_eq!(
        exec.handle_poll_command(&sandbox, "lease-1", id.as_str()).await,
        PollCommandResponse::Running
    );

    server.reset().await;
    mount_file(&server, "/exit", Some("0\n")).await;
    mount_file(&server, "/out", Some("done")).await;
    mount_file(&server, "/err", None).await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    let PollCommandResponse::Completed(output) = exec.handle_poll_command(&sandbox, "lease-1", id.as_str()).await
    else {
        return Err("the command must have completed".into());
    };
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"done");
    assert!(output.stderr.is_empty());
    assert_eq!(
        exec.handle_poll_command(&sandbox, "other-lease", id.as_str()).await,
        PollCommandResponse::NotFound
    );
    assert_eq!(
        exec.handle_poll_command(&sandbox, "lease-1", "nope").await,
        PollCommandResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn kill_signals_a_running_command_and_reports_a_finished_one() -> TestResult {
    let server = MockServer::start().await;
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    let (exec, sandbox) = fixture(&server)?;
    let id = started(&exec, &sandbox, &detached(&["sleep", "60"])).await?;
    let kill = KillCommandRequest {
        lease_id: LeaseId::new("lease-1"),
        execution_id: id,
        signal: UnixSignal::Kill,
    };

    mount_file(&server, "/exit", None).await;
    mount_script(&server, super::constants::KILL_SCRIPT, exited(0)?).await;
    assert_eq!(
        exec.handle_kill_command(&sandbox, "lease-1", &kill).await,
        KillCommandResponse::Signaled
    );

    server.reset().await;
    mount_file(&server, "/exit", Some("137")).await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    assert_eq!(
        exec.handle_kill_command(&sandbox, "lease-1", &kill).await,
        KillCommandResponse::AlreadyCompleted
    );
    let unknown = KillCommandRequest {
        execution_id: ExecutionId::new("nope"),
        ..kill
    };
    assert_eq!(
        exec.handle_kill_command(&sandbox, "lease-1", &unknown).await,
        KillCommandResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn forgetting_a_lease_drops_its_executions() -> TestResult {
    let server = MockServer::start().await;
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    let (exec, sandbox) = fixture(&server)?;
    let id = started(&exec, &sandbox, &detached(&["x"])).await?;
    exec.forget_lease("lease-1").await;
    assert_eq!(
        exec.handle_poll_command(&sandbox, "lease-1", id.as_str()).await,
        PollCommandResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn files_round_trip_through_envd() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    mount_script(&server, super::constants::CHMOD_SCRIPT, exited(0)?).await;
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
    assert_eq!(
        exec.handle_write_files(&sandbox, &write).await,
        WriteFilesResponse::Written
    );

    mount_file(&server, "/w/a.txt", Some("hi")).await;
    mount_file(&server, "/w/missing", None).await;
    let read = ReadFileRequest {
        lease_id: LeaseId::new("lease-1"),
        path: "/w/a.txt".to_owned(),
        as_user: None,
    };
    assert_eq!(
        exec.handle_read_file(&sandbox, &read).await,
        ReadFileResponse::Content { data: b"hi".to_vec() }
    );
    let gone = ReadFileRequest {
        path: "/w/missing".to_owned(),
        ..read
    };
    assert_eq!(exec.handle_read_file(&sandbox, &gone).await, ReadFileResponse::NotFound);
    Ok(())
}

#[tokio::test]
async fn a_failed_upload_or_chmod_is_denied() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"code": 500, "message": "disk full"})))
        .mount(&server)
        .await;
    let (exec, sandbox) = fixture(&server)?;
    let write = WriteFilesRequest {
        lease_id: LeaseId::new("lease-1"),
        files: vec![SandboxFileWrite {
            path: "/w/a.txt".to_owned(),
            content: vec![],
            mode: None,
        }],
        as_user: None,
    };
    assert!(matches!(
        exec.handle_write_files(&sandbox, &write).await,
        WriteFilesResponse::Denied(LeaseError::Backend { .. })
    ));
    Mock::given(method("GET"))
        .and(path("/files"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"code": 500, "message": "io"})))
        .mount(&server)
        .await;
    let read = ReadFileRequest {
        lease_id: LeaseId::new("lease-1"),
        path: "/w/a.txt".to_owned(),
        as_user: None,
    };
    assert!(matches!(
        exec.handle_read_file(&sandbox, &read).await,
        ReadFileResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn the_preview_url_is_port_dash_sandbox_dot_domain() -> TestResult {
    let server = MockServer::start().await;
    let (exec, mut sandbox) = fixture(&server)?;
    let trogon_sandbox_contract::GetSandboxDomainResponse::Available { endpoint, access, .. } =
        exec.handle_get_domain(&sandbox, 3000)
    else {
        return Err("the port must be available".into());
    };
    assert_eq!(endpoint, "3000-sbx1.e2b.app:443");
    let access = access.ok_or("access is missing")?;
    assert_eq!(access.base_url, "https://3000-sbx1.e2b.app");
    assert!(access.headers.is_empty());

    sandbox.traffic_token = Some("tt".to_owned());
    let trogon_sandbox_contract::GetSandboxDomainResponse::Available { access, .. } =
        exec.handle_get_domain(&sandbox, 3000)
    else {
        return Err("the port must be available".into());
    };
    assert_eq!(
        access
            .ok_or("access is missing")?
            .headers
            .get(TRAFFIC_TOKEN_HEADER)
            .map(String::as_str),
        Some("tt")
    );
    assert!(matches!(
        exec.handle_get_domain(&sandbox, 0),
        trogon_sandbox_contract::GetSandboxDomainResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn stdin_is_unsupported() -> TestResult {
    let server = MockServer::start().await;
    let (exec, _) = fixture(&server)?;
    assert!(matches!(exec.handle_write_stdin(), WriteStdinResponse::Unsupported));
    Ok(())
}

#[tokio::test]
async fn configuring_twice_is_refused() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    exec.configure_command_launches(context.clone())?;
    assert!(exec.command_custody.is_configured());
    assert!(matches!(
        exec.configure_command_launches(context),
        Err(ExecError::CustodyAlreadyConfigured)
    ));
    Ok(())
}

fn owner(lease: &str, sandbox: &str) -> Result<CommandOwner, Box<dyn std::error::Error>> {
    Ok(CommandOwner {
        tenant: "tenant-1".parse()?,
        lease_id: LeaseId::new(lease),
        sandbox_id: sandbox.to_owned(),
        worker_id: "worker-1".parse()?,
        attempt_id: "attempt-1".parse()?,
        epoch: 1u64.try_into()?,
    })
}

#[test]
fn an_owner_needs_a_sane_lease_and_sandbox_identity() -> TestResult {
    assert!(owner("lease-1", "sbx-1")?.validate().is_ok());
    assert!(matches!(
        owner("", "sbx")?.validate(),
        Err(CommandOwnerError::LeaseIdentity)
    ));
    assert!(matches!(
        owner("a\nb", "sbx")?.validate(),
        Err(CommandOwnerError::LeaseIdentity)
    ));
    assert!(matches!(
        owner("l", "")?.validate(),
        Err(CommandOwnerError::SandboxIdentity)
    ));
    assert!(matches!(
        owner("l", "a b")?.validate(),
        Err(CommandOwnerError::SandboxIdentity)
    ));
    Ok(())
}

fn record() -> Result<CommandRecord, Box<dyn std::error::Error>> {
    let request = run_request(&["x"]);
    Ok(CommandRecord::reserved(
        owner("lease-1", "sbx-1")?,
        LaunchKey::new("key-1")?,
        LaunchFingerprint::of(&request)?,
    ))
}

#[test]
fn a_reserved_record_encodes_and_a_broken_one_does_not() -> TestResult {
    let mut reserved = record()?;
    assert!(reserved.encode().is_ok());
    reserved.version = 99;
    assert!(matches!(reserved.validate(), Err(CommandRecordError::Version)));
    let mut reserved = record()?;
    reserved.execution_id = None;
    assert!(matches!(reserved.validate(), Err(CommandRecordError::PhaseAuthority)));
    let mut reserved = record()?;
    reserved.execution_id = Some(ExecutionId::new("not-a-ulid"));
    assert!(reserved.validate().is_err());
    let canceled = CommandRecord::canceled_before_start(owner("lease-1", "sbx-1")?, LaunchKey::new("key-1")?);
    assert!(canceled.encode().is_ok());
    Ok(())
}

#[test]
fn a_launch_fingerprint_ignores_the_key_and_redactions() -> TestResult {
    let plain = LaunchFingerprint::of(&run_request(&["x"]))?;
    let keyed = RunCommandRequest {
        launch_key: Some(LaunchKey::new("k")?),
        ..run_request(&["x"])
    };
    assert_eq!(plain, LaunchFingerprint::of(&keyed)?);
    assert_ne!(plain, LaunchFingerprint::of(&run_request(&["y"]))?);
    assert!(LaunchFingerprint::try_from("zz".to_owned()).is_err());
    assert_eq!(String::from(plain).len(), 64);
    Ok(())
}

#[tokio::test]
async fn custody_creates_loads_and_replaces_records() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let custody = CommandCustody::default();
    let owner = owner("lease-1", "sbx-1")?;
    let key = LaunchKey::new("key-1")?;
    assert!(custody.load(&owner, &key).await.is_err());
    custody.configure(server.jetstream().await)?;
    assert!(custody.load(&owner, &key).await?.is_none());

    let reserved = record()?;
    let stored = custody
        .create(reserved.clone())
        .await?
        .ok_or("the record must be created")?;
    assert!(custody.create(reserved.clone()).await?.is_none());
    let loaded = custody.load(&owner, &key).await?.ok_or("the record must exist")?;
    assert_eq!(loaded.revision, stored.revision);

    let mut next = reserved.clone();
    next.cancel_requested = true;
    next.phase = super::command_phase::CommandPhase::Canceled;
    let replaced = custody
        .replace(&stored, next.clone())
        .await?
        .ok_or("the record must be replaced")?;
    assert!(custody.replace(&stored, next).await?.is_none());
    assert!(replaced.record.cancel_requested);

    let mut other = reserved;
    other.execution_id = Some(Execution::fresh_id());
    assert!(custody.replace(&replaced, other).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_clock_counts_down_and_restores_within_its_budget() -> TestResult {
    let clock = Clock::new(30)?;
    assert!(clock.remaining_ms()? <= 30_000);
    assert!(clock.expires_unix_ms >= clock.issued_unix_ms);
    let now = Clock::now_unix_ms()?;
    let restored = Clock::restore(now - 10, now + 5_000, now - 5, 2_000)?;
    assert!(restored.remaining_ms()? <= 2_000);
    assert!(Clock::restore(now + 60_000, now + 90_000, now - 5, 1_000).is_err());
    assert!(Clock::restore(now - 10, now + 5_000, now + 60_000, 1_000).is_err());
    Ok(())
}

#[test]
fn failure_helpers_keep_the_launch_uncertainty() {
    let error = ApiError::Transport("boom".to_owned());
    assert!(matches!(denied(&error), LeaseError::Backend { .. }));
    assert!(matches!(invalid_spec("no"), LeaseError::InvalidSpec { .. }));
    assert!(matches!(custody_unavailable(), LeaseError::Backend { .. }));
    assert!(matches!(after_launch(invalid_spec("no")), LeaseError::Backend { .. }));
    assert!(matches!(after_launch(denied(&error)), LeaseError::Backend { .. }));
}

#[test]
fn an_execution_directory_is_derived_from_a_fresh_identity() {
    let id = Execution::fresh_id();
    assert!(Execution::directory(&id).starts_with(EXECUTION_DIRECTORY_PREFIX));
    assert_ne!(id, Execution::fresh_id());
}

#[test]
fn the_contract_types_reject_reserved_environment_keys_and_empty_tenants() {
    let reserved = std::collections::BTreeMap::from([(
        trogon_sandbox_contract::CommandEnv::RESERVED_KEY.to_owned(),
        "x".to_owned(),
    )]);
    let ordinary = std::collections::BTreeMap::from([("PATH".to_owned(), "/bin".to_owned())]);
    assert!(trogon_sandbox_contract::CommandEnv::new(ordinary).is_ok());
    assert!(trogon_sandbox_contract::CommandEnv::new(reserved).is_err());
    assert!(trogon_sandbox_contract::TenantId::new("").is_err());
}

#[tokio::test]
async fn an_expiring_custody_bucket_is_refused_on_open_and_after_a_change() -> TestResult {
    let owner = owner("lease-1", "sbx-1")?;
    let key = LaunchKey::new("key-1")?;
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: super::constants::COMMAND_CUSTODY_BUCKET.into(),
            history: 1,
            max_age: Duration::from_secs(600),
            ..Default::default()
        })
        .await?;
    let custody = CommandCustody::default();
    custody.configure(context)?;
    assert!(matches!(
        custody.load(&owner, &key).await,
        Err(super::command_custody_error::CommandCustodyError::Bucket(
            crate::CustodyBucketError::NotDurable
        ))
    ));

    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let custody = CommandCustody::default();
    custody.configure(context.clone())?;
    assert!(custody.load(&owner, &key).await?.is_none());
    let mut config = context
        .get_stream(format!("KV_{}", super::constants::COMMAND_CUSTODY_BUCKET))
        .await?
        .cached_info()
        .config
        .clone();
    config.max_age = Duration::from_secs(600);
    context.update_stream(config).await?;
    assert!(matches!(
        custody.load(&owner, &key).await,
        Err(super::command_custody_error::CommandCustodyError::Bucket(
            crate::CustodyBucketError::NotDurable
        ))
    ));
    Ok(())
}
