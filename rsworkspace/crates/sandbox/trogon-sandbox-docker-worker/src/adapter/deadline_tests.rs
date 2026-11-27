use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, MutexGuard, PoisonError};

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_providers::{
    CheckpointCreateSpec, CheckpointRestoreSpec, ContainerStartResult, DockerEngine, DockerError,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::support_tests::{TestResult, lease_request};

type FixtureError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Copy)]
enum SignalReply {
    ApiError,
    Nonzero,
    Success,
}

struct RemoteExecution {
    running: AtomicBool,
    started: Notify,
    hold_start: AtomicBool,
    start_entered: Notify,
    allow_start: CancellationToken,
    release: CancellationToken,
    lose_stream: CancellationToken,
    stdout: StdMutex<Vec<u8>>,
    inspection_observed: Notify,
    marker_attempts: AtomicUsize,
    pid_attempts: AtomicUsize,
    exit_during_pid_signal: AtomicBool,
    signal_exit_reads: AtomicUsize,
    signal_reply: StdMutex<SignalReply>,
    inspection_exit: StdMutex<Option<i32>>,
    cleanup_fails: AtomicBool,
    cleanup_ack_only: AtomicBool,
    resource_present: AtomicBool,
    cleanup_attempts: AtomicUsize,
}

fn locked<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RemoteExecution {
    fn signal_reply(&self) -> SignalReply {
        *locked(&self.signal_reply)
    }
}

struct DockerApi {
    endpoint: String,
    remote: Arc<RemoteExecution>,
    stop: CancellationToken,
    task: Option<JoinHandle<Result<(), FixtureError>>>,
}

impl Drop for DockerApi {
    fn drop(&mut self) {
        self.remote.release.cancel();
        self.stop.cancel();
    }
}

impl DockerApi {
    async fn start(signal_reply: SignalReply) -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let remote = Arc::new(RemoteExecution {
            running: AtomicBool::new(false),
            started: Notify::new(),
            hold_start: AtomicBool::new(false),
            start_entered: Notify::new(),
            allow_start: CancellationToken::new(),
            release: CancellationToken::new(),
            lose_stream: CancellationToken::new(),
            stdout: StdMutex::new(Vec::new()),
            inspection_observed: Notify::new(),
            marker_attempts: AtomicUsize::new(0),
            pid_attempts: AtomicUsize::new(0),
            exit_during_pid_signal: AtomicBool::new(false),
            signal_exit_reads: AtomicUsize::new(0),
            signal_reply: StdMutex::new(signal_reply),
            inspection_exit: StdMutex::new(None),
            cleanup_fails: AtomicBool::new(false),
            cleanup_ack_only: AtomicBool::new(false),
            resource_present: AtomicBool::new(true),
            cleanup_attempts: AtomicUsize::new(0),
        });
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let remote = remote.clone();
            let stop = stop.clone();
            async move {
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        _ = stop.cancelled() => break,
                        accepted = listener.accept() => {
                            let (stream, _) = accepted?;
                            connections.spawn(serve(stream, remote.clone()));
                        }
                    }
                }
                while let Some(result) = connections.join_next().await {
                    result??;
                }
                Ok(())
            }
        });
        Ok(Self {
            endpoint,
            remote,
            stop,
            task: Some(task),
        })
    }

    async fn shutdown(mut self) -> TestResult {
        self.remote.release.cancel();
        self.stop.cancel();
        match self.task.take() {
            Some(task) => task.await?.map_err(|error| -> Box<dyn Error> { error }),
            None => Ok(()),
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<(String, serde_json::Value), FixtureError> {
    let mut bytes = Vec::new();
    let (header_end, content_len) = loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Err("the Docker client must send its request".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let content_len = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            break (end + 4, content_len);
        }
    };
    while bytes.len() < header_end + content_len {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            return Err("the Docker client closed before sending its body".into());
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let path = String::from_utf8_lossy(&bytes[..header_end])
        .split_whitespace()
        .nth(1)
        .ok_or("the request line has no path")?
        .to_string();
    let body = if content_len == 0 {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + content_len])?
    };
    Ok((path, body))
}

async fn json_reply(stream: &mut TcpStream, status: &str, body: serde_json::Value) -> Result<(), FixtureError> {
    let body = serde_json::to_vec(&body)?;
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

async fn serve(mut stream: TcpStream, remote: Arc<RemoteExecution>) -> Result<(), FixtureError> {
    let (path, body) = read_request(&mut stream).await?;
    let path = path.split('?').next().unwrap_or_default();
    if path.ends_with("/containers/command-fixture/exec") {
        let command = body["Cmd"].as_array().ok_or("exec create has no command")?;
        if command.first().and_then(|value| value.as_str()) == Some("held-command") {
            return json_reply(&mut stream, "201 Created", serde_json::json!({"Id":"held"})).await;
        }
        let is_pid = command.first().and_then(|value| value.as_str()) == Some("kill");
        if is_pid {
            remote.pid_attempts.fetch_add(1, Ordering::SeqCst);
            if remote.exit_during_pid_signal.load(Ordering::SeqCst) {
                *locked(&remote.inspection_exit) = Some(143);
                remote.release.cancel();
            }
        } else {
            if !command
                .iter()
                .any(|arg| arg.as_str().is_some_and(|arg| arg.contains("/proc/[0-9]*/environ")))
            {
                return Err("the marker lookup must read the process environments".into());
            }
            remote.marker_attempts.fetch_add(1, Ordering::SeqCst);
        }
        match remote.signal_reply() {
            SignalReply::ApiError => {
                json_reply(
                    &mut stream,
                    "503 Service Unavailable",
                    serde_json::json!({"message":"signal exec unavailable"}),
                )
                .await
            }
            _ => json_reply(&mut stream, "201 Created", serde_json::json!({"Id":"signal"})).await,
        }
    } else if path.ends_with("/exec/held/start") || path.ends_with("/exec/signal/start") {
        if path.ends_with("/exec/held/start") && remote.hold_start.load(Ordering::SeqCst) {
            remote.start_entered.notify_one();
            remote.allow_start.cancelled().await;
        }
        stream
            .write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: tcp\r\n\r\n")
            .await?;
        if path.ends_with("/exec/held/start") {
            remote.running.store(true, Ordering::SeqCst);
            remote.started.notify_one();
            tokio::select! {
                _ = remote.release.cancelled() => {},
                _ = remote.lose_stream.cancelled() => {
                    drop(stream);
                    remote.release.cancelled().await;
                    remote.running.store(false, Ordering::SeqCst);
                    return Ok(());
                }
            }
            let output = locked(&remote.stdout).clone();
            if !output.is_empty() {
                let mut header = vec![1, 0, 0, 0];
                header.extend_from_slice(&u32::try_from(output.len())?.to_be_bytes());
                stream.write_all(&header).await?;
                stream.write_all(&output).await?;
            }
            remote.running.store(false, Ordering::SeqCst);
        } else if matches!(remote.signal_reply(), SignalReply::Success) {
            remote.release.cancel();
        }
        Ok(())
    } else if path.ends_with("/exec/held/json") {
        let native_exit = *locked(&remote.inspection_exit);
        remote.inspection_observed.notify_one();
        json_reply(
            &mut stream,
            "200 OK",
            serde_json::json!({
                "ID":"held", "Running":native_exit.is_none() && remote.running.load(Ordering::SeqCst),
                "Pid":4242, "ExitCode":native_exit.unwrap_or(137),
            }),
        )
        .await
    } else if path.ends_with("/exec/signal/json") {
        remote.signal_exit_reads.fetch_add(1, Ordering::SeqCst);
        json_reply(
            &mut stream,
            "200 OK",
            serde_json::json!({
                "ID":"signal", "Running":false,
                "ExitCode":if matches!(remote.signal_reply(), SignalReply::Success) {0} else {1},
            }),
        )
        .await
    } else if path.ends_with("/containers/command-fixture/json") {
        if remote.resource_present.load(Ordering::SeqCst) {
            json_reply(
                &mut stream,
                "200 OK",
                serde_json::json!({
                    "Id":"command-fixture",
                    "State":{"Paused":false,"Running":remote.running.load(Ordering::SeqCst)},
                }),
            )
            .await
        } else {
            json_reply(
                &mut stream,
                "404 Not Found",
                serde_json::json!({"message":"No such container: command-fixture"}),
            )
            .await
        }
    } else {
        Err(format!("unexpected Docker API route: {path}").into())
    }
}

fn refused(operation: &'static str) -> DockerError {
    DockerError::Api {
        operation,
        source: bollard::errors::Error::DockerResponseServerError {
            status_code: 500,
            message: format!("fixture refused {operation}"),
        },
    }
}

struct CommandEngine(Arc<RemoteExecution>);

#[async_trait::async_trait]
impl DockerEngine for CommandEngine {
    async fn create_and_start(&self, _: ContainerRunSpec) -> Result<ContainerStartResult, DockerError> {
        Err(refused("allocation"))
    }
    async fn inspect(&self, _: &str) -> Result<InspectStatus, DockerError> {
        Err(refused("container"))
    }
    async fn kill(&self, _: &str) -> Result<(), DockerError> {
        self.0.cleanup_attempts.fetch_add(1, Ordering::SeqCst);
        if self.0.cleanup_fails.load(Ordering::SeqCst) {
            return Err(refused("kill"));
        }
        if self.0.cleanup_ack_only.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.0.release.cancel();
        while self.0.running.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        self.0.resource_present.store(false, Ordering::SeqCst);
        Ok(())
    }
    async fn commit(&self, _: &str, _: &str, _: &str) -> Result<String, DockerError> {
        Err(refused("commit"))
    }
    async fn export_rootfs(&self, _: &str) -> Result<Vec<u8>, DockerError> {
        Err(refused("export"))
    }
    async fn import_rootfs(&self, _: &[u8], _: &str, _: &str) -> Result<String, DockerError> {
        Err(refused("import"))
    }
    async fn create_checkpoint_leave_running(&self, _: &str, _: &CheckpointCreateSpec) -> Result<(), DockerError> {
        Err(refused("checkpoint"))
    }
    async fn start_from_checkpoint(&self, _: &str, _: &CheckpointRestoreSpec) -> Result<(), DockerError> {
        Err(refused("restore"))
    }
    async fn create_container_stopped(&self, _: ContainerRunSpec) -> Result<String, DockerError> {
        Err(refused("create"))
    }
    async fn list_by_label(&self, _: &str, _: &str) -> Result<Vec<String>, DockerError> {
        Err(refused("list"))
    }
    async fn remove(&self, _: &str) -> Result<(), DockerError> {
        Err(refused("remove"))
    }
}

async fn provider(
    api: &DockerApi,
    client: async_nats::Client,
) -> Result<(DockerWorkerProvider, LeaseId), Box<dyn Error>> {
    let mut provider = DockerWorkerProvider::new(
        LocalDockerProvider::new(Arc::new(CommandEngine(api.remote.clone())), "unused".into()),
        "unused".into(),
        jetstream::new(client),
        "unused".into(),
        "command-fixture".into(),
        build_oci_client(vec![]),
        "docker.io".into(),
        None,
        Arc::new(DigestCache::new(8)),
        None,
        None,
        default_warm_pool_config(),
    )?;
    provider.docker = Docker::connect_with_http(&api.endpoint, 30, bollard::API_DEFAULT_VERSION)?;
    let lease_id = LeaseId::new("command-fixture");
    provider.active.lock().await.insert(
        lease_id.as_str().to_owned(),
        ActiveDockerLease {
            runtime_mount: None,
            registration_id: crate::fresh_id::fresh_id(),
            dispatch_target: None,
            dispatch_target_bound: false,
            container_id: "command-fixture".into(),
            snapshot_digest: None,
            snapshot_reference: None,
            memory_mib: 128,
            cpu: 1.0,
            policy_file_path: None,
            sidecar_id: None,
            exposed_port_endpoints: HashMap::new(),
            warm_pool_eligible: false,
            request: Box::new(lease_request("command-fixture")?),
        },
    );
    Ok((provider, lease_id))
}

async fn detached(
    provider: &DockerWorkerProvider,
    lease: &LeaseId,
    timeout: u32,
) -> Result<ExecutionId, Box<dyn Error>> {
    Ok(provider
        .run_command_for_lease_detached(
            lease,
            vec!["held-command".into()],
            Some("/".into()),
            &CommandEnv::default(),
            timeout,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            1024,
            None,
        )
        .await?)
}

/// Runs `probe` every 10ms until it yields a value, failing once ten seconds have passed.
async fn until<T, F, Fut>(mut probe: F) -> Result<T, Box<dyn Error>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tick.tick().await;
            if let Some(value) = probe().await {
                break value;
            }
        }
    })
    .await
    .map_err(Into::into)
}

async fn wait_completed(
    provider: &DockerWorkerProvider,
    lease: &LeaseId,
    execution: &ExecutionId,
) -> Result<CommandOutput, Box<dyn Error>> {
    until(|| async {
        match provider.poll_command_for_lease(lease, execution).await {
            Some(PollCommandResponse::Completed(output)) => Some(output),
            _ => None,
        }
    })
    .await
}

async fn settle_remote(
    provider: &DockerWorkerProvider,
    lease: &LeaseId,
    execution: &ExecutionId,
    api: &DockerApi,
) -> TestResult {
    let task = match provider.executions.lock().await.get(execution) {
        Some(ExecutionState::Running { task, .. }) => task.clone(),
        _ => None,
    };
    api.remote.release.cancel();
    wait_completed(provider, lease, execution).await?;
    if let Some(task) = task {
        task.join().await;
    }
    Ok(())
}

async fn wait_uncertain(provider: &DockerWorkerProvider, lease: &LeaseId, execution: &ExecutionId) -> TestResult {
    until(|| async {
        matches!(
            provider.poll_command_for_lease(lease, execution).await,
            Some(PollCommandResponse::Denied(LeaseError::Backend { .. }))
        )
        .then_some(())
    })
    .await
}

async fn retained_task(
    provider: &DockerWorkerProvider,
    execution: &ExecutionId,
) -> Result<Arc<CommandTask>, Box<dyn Error>> {
    match provider.executions.lock().await.get(execution) {
        Some(ExecutionState::Running { task: Some(task), .. }) => Ok(task.clone()),
        _ => Err("uncertain execution must retain its collector".into()),
    }
}

async fn foreground(provider: &DockerWorkerProvider, lease: &LeaseId) -> anyhow::Result<CommandOutput> {
    provider
        .run_command_for_lease(
            lease,
            vec!["held-command".into()],
            Some("/".into()),
            &CommandEnv::default(),
            1,
            None,
            StreamedOutput::Disabled,
            1024,
        )
        .await
}

#[tokio::test]
async fn foreground_timeout_does_not_claim_completion_when_both_kill_paths_fail() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let result = foreground(&provider, &lease).await;
    let still_running = api.remote.running.load(Ordering::SeqCst);
    let attempts = (
        api.remote.marker_attempts.load(Ordering::SeqCst),
        api.remote.pid_attempts.load(Ordering::SeqCst),
    );
    let execution = provider
        .executions
        .lock()
        .await
        .keys()
        .next()
        .ok_or("the timed out command stays tracked")?
        .clone();
    settle_remote(&provider, &lease, &execution, &api).await?;
    api.shutdown().await?;
    assert!(still_running);
    assert_eq!(attempts, (1, 1));
    assert!(result.is_err(), "the command may still be running: {result:?}");
    Ok(())
}

#[tokio::test]
async fn detached_timeout_does_not_claim_completion_when_both_kill_paths_fail() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 1).await?;
    api.remote.started.notified().await;
    let polled = until(|| async {
        let state = provider.poll_command_for_lease(&lease, &execution).await;
        (!matches!(state, Some(PollCommandResponse::Running))).then_some(state)
    })
    .await?;
    let still_running = api.remote.running.load(Ordering::SeqCst);
    let attempts = (
        api.remote.marker_attempts.load(Ordering::SeqCst),
        api.remote.pid_attempts.load(Ordering::SeqCst),
    );
    settle_remote(&provider, &lease, &execution, &api).await?;
    api.shutdown().await?;
    assert!(still_running);
    assert_eq!(attempts, (1, 1));
    assert!(
        matches!(polled, Some(PollCommandResponse::Denied(LeaseError::Backend { .. }))),
        "the command may still be running: {polled:?}"
    );
    Ok(())
}

async fn failed_signal_does_not_report_signaled(reply: SignalReply) -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(reply).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 60).await?;
    api.remote.started.notified().await;
    let response = provider
        .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
        .await;
    let still_running = api.remote.running.load(Ordering::SeqCst);
    let attempts = (
        api.remote.marker_attempts.load(Ordering::SeqCst),
        api.remote.pid_attempts.load(Ordering::SeqCst),
    );
    let signal_exit_reads = api.remote.signal_exit_reads.load(Ordering::SeqCst);
    settle_remote(&provider, &lease, &execution, &api).await?;
    api.shutdown().await?;
    assert!(still_running);
    assert_eq!(attempts, (1, 1));
    if matches!(reply, SignalReply::Nonzero) {
        assert_eq!(signal_exit_reads, 2, "both signal exit codes were inspected");
    }
    assert!(
        matches!(response, KillCommandResponse::Denied(LeaseError::Backend { .. })),
        "failed signal delivery must remain an explicit uncertain error: {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn kill_command_does_not_report_signaled_after_api_failures() -> TestResult {
    failed_signal_does_not_report_signaled(SignalReply::ApiError).await
}

#[tokio::test]
async fn kill_command_does_not_report_signaled_after_nonzero_kill_exit() -> TestResult {
    failed_signal_does_not_report_signaled(SignalReply::Nonzero).await
}

#[tokio::test]
async fn a_command_that_exits_during_failed_pid_signaling_is_already_completed() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::Nonzero).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 60).await?;
    api.remote.started.notified().await;
    api.remote.exit_during_pid_signal.store(true, Ordering::SeqCst);
    let response = provider
        .kill_command_for_lease(&lease, &execution, UnixSignal::Term)
        .await;
    let attempts = (
        api.remote.marker_attempts.load(Ordering::SeqCst),
        api.remote.pid_attempts.load(Ordering::SeqCst),
    );
    settle_remote(&provider, &lease, &execution, &api).await?;
    api.shutdown().await?;
    assert_eq!(attempts, (1, 1));
    assert!(
        matches!(response, KillCommandResponse::AlreadyCompleted),
        "the exact Docker execution exited while its signal command failed: {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn kill_waits_for_exec_start_before_signaling_the_command() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::Success).await?;
    api.remote.hold_start.store(true, Ordering::SeqCst);
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 60).await?;
    api.remote.start_entered.notified().await;
    let kill = provider.kill_command_for_lease(&lease, &execution, UnixSignal::Kill);
    tokio::pin!(kill);
    let early = tokio::time::timeout(Duration::from_millis(500), &mut kill).await;
    let premature = early.is_ok();
    let early_marker_attempts = api.remote.marker_attempts.load(Ordering::SeqCst);
    api.remote.allow_start.cancel();
    let response = match early {
        Ok(response) => response,
        Err(_) => tokio::time::timeout(Duration::from_secs(5), &mut kill).await?,
    };
    settle_remote(&provider, &lease, &execution, &api).await?;
    api.shutdown().await?;
    assert!(!premature, "a created Docker exec has not started: {response:?}");
    assert_eq!(early_marker_attempts, 0, "an unstarted exec must not be signaled");
    assert!(matches!(response, KillCommandResponse::Signaled));
    Ok(())
}

#[tokio::test]
async fn confirmed_kill_preserves_foreground_timeout_output() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::Success).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let result = foreground(&provider, &lease).await;
    let signal_exit_reads = api.remote.signal_exit_reads.load(Ordering::SeqCst);
    api.shutdown().await?;
    let output = result?;
    assert_eq!(signal_exit_reads, 1, "the successful signal exit was inspected");
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    assert_eq!(output.exit_code, -1);
    assert_eq!(output.stderr, b"command timed out after 1s");
    Ok(())
}

#[tokio::test]
async fn failed_timeout_keeps_the_original_collector_until_a_successful_kill_retry() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 1).await?;
    wait_uncertain(&provider, &lease, &execution).await?;
    let task = retained_task(&provider, &execution).await?;
    let retained_stream = !task.is_finished() && api.remote.running.load(Ordering::SeqCst);
    *locked(&api.remote.signal_reply) = SignalReply::Success;
    let retry = provider
        .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
        .await;
    let output = wait_completed(&provider, &lease, &execution).await?;
    task.join().await;
    let settled = task.is_finished() && !api.remote.running.load(Ordering::SeqCst);
    api.shutdown().await?;
    assert!(
        retained_stream,
        "the failed timeout must retain its original stream owner"
    );
    assert_eq!(retry, KillCommandResponse::Signaled);
    assert!(settled);
    assert_eq!(output.exit_code, 137);
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    Ok(())
}

#[tokio::test]
async fn failed_lease_cleanup_retains_uncertain_execution_until_confirmed_disposal() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 1).await?;
    wait_uncertain(&provider, &lease, &execution).await?;
    let task = retained_task(&provider, &execution).await?;
    api.remote.cleanup_fails.store(true, Ordering::SeqCst);
    let first = provider.cancel(&lease).await;
    let retained = provider.executions.lock().await.contains_key(&execution) && !task.is_finished();
    api.remote.cleanup_fails.store(false, Ordering::SeqCst);
    let retry = provider.cancel(&lease).await;
    let forgotten = !provider.executions.lock().await.contains_key(&execution);
    let settled = task.is_finished() && !api.remote.running.load(Ordering::SeqCst);
    let attempts = api.remote.cleanup_attempts.load(Ordering::SeqCst);
    api.shutdown().await?;
    assert!(first.is_err());
    assert!(retained, "failed disposal must retain the command and its stream owner");
    retry?;
    assert!(forgotten && settled, "confirmed lease disposal must join its collector");
    assert_eq!(attempts, 2);
    Ok(())
}

#[tokio::test]
async fn delayed_collection_preserves_an_already_exited_native_status() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    *locked(&api.remote.inspection_exit) = Some(124);
    *locked(&api.remote.stdout) = b"late-output-sentinel\n".to_vec();
    let (provider, lease) = provider(&api, server.client().await).await?;
    let run = foreground(&provider, &lease);
    let (result, stream_was_delayed) = tokio::join!(run, async {
        api.remote.inspection_observed.notified().await;
        let was_held = api.remote.running.load(Ordering::SeqCst);
        api.remote.release.cancel();
        was_held
    });
    let pid_attempts = api.remote.pid_attempts.load(Ordering::SeqCst);
    api.shutdown().await?;
    assert!(
        stream_was_delayed,
        "the output stream must lag behind native exit observation"
    );
    let output = result?;
    assert_eq!(output.exit_code, 124);
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(output.stdout, b"late-output-sentinel\n");
    assert_eq!(pid_attempts, 0);
    Ok(())
}

#[tokio::test]
async fn kill_after_a_failed_collector_preserves_output_uncertainty_until_lease_disposal() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 60).await?;
    api.remote.started.notified().await;
    let task = retained_task(&provider, &execution).await?;
    api.remote.lose_stream.cancel();
    wait_uncertain(&provider, &lease, &execution).await?;
    task.join().await;
    let collector_failed_while_running = task.is_finished() && api.remote.running.load(Ordering::SeqCst);
    *locked(&api.remote.signal_reply) = SignalReply::Success;
    let kill = provider
        .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
        .await;
    let result = provider.poll_command_for_lease(&lease, &execution).await;
    let cleanup = provider.cancel(&lease).await;
    let forgotten = !provider.executions.lock().await.contains_key(&execution);
    api.shutdown().await?;
    assert!(collector_failed_while_running);
    assert_eq!(kill, KillCommandResponse::Signaled);
    assert!(
        matches!(result, Some(PollCommandResponse::Denied(LeaseError::Backend { .. }))),
        "process exit cannot invent unavailable output: {result:?}"
    );
    cleanup?;
    assert!(forgotten);
    Ok(())
}

#[tokio::test]
async fn accepted_container_kill_retains_custody_until_exact_resource_absence() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let api = DockerApi::start(SignalReply::ApiError).await?;
    let (provider, lease) = provider(&api, server.client().await).await?;
    let execution = detached(&provider, &lease, 60).await?;
    api.remote.started.notified().await;
    let task = retained_task(&provider, &execution).await?;
    api.remote.cleanup_ack_only.store(true, Ordering::SeqCst);
    let result = provider.cancel(&lease).await;
    let actual_resource = provider.docker.inspect_container("command-fixture", None).await?;
    let native = provider.native_handle_for_lease(&lease).await;
    let retained_collector = !task.is_finished();
    let still_running = api.remote.running.load(Ordering::SeqCst);
    api.remote.cleanup_ack_only.store(false, Ordering::SeqCst);
    api.remote.resource_present.store(false, Ordering::SeqCst);
    api.remote.release.cancel();
    until(|| async { (!api.remote.running.load(Ordering::SeqCst)).then_some(()) }).await?;
    let retry = provider.cancel(&lease).await;
    task.join().await;
    let forgotten = !provider.executions.lock().await.contains_key(&execution);
    let absent = matches!(
        provider.docker.inspect_container("command-fixture", None).await,
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
    );
    api.shutdown().await?;
    assert_eq!(actual_resource.id.as_deref(), Some("command-fixture"));
    assert!(
        still_running,
        "accepted kill must not stand in for actual resource termination"
    );
    assert!(
        result.is_err(),
        "an accepted kill with an extant exact resource must remain uncertain"
    );
    assert!(
        matches!(native, trogon_sandbox_contract::NativeHandleResponse::Available(handle) if handle.vendor_id == "command-fixture"),
        "the exact lease resource receipt must survive unconfirmed cleanup"
    );
    assert!(
        retained_collector,
        "unconfirmed cleanup must retain the original stream owner"
    );
    retry?;
    assert!(forgotten && absent && task.is_finished());
    Ok(())
}
