use super::*;
use crate::command_launch::CommandLaunchError;
use trogon_sandbox_contract::{
    CommandExecution, IsolationCapability, LaunchCancellation, LaunchKey, RedactValues, StdinChunk, StdinEof,
    StdinMode, StdinSeq, StreamedOutput, TerminalDimension, WorkerDispatchProtocol,
};

/// The execution id every command helper here answers with.
const EXEC_ID: &str = "exec-1";

/// A request to run `command` on `lease_id` that waits for completion, with
/// every optional field at its default.
pub(super) fn base_run_request(lease_id: LeaseId, command: &[&str]) -> RunCommandRequest {
    RunCommandRequest {
        require_launch_cancellation: LaunchCancellation::NotRequired,
        launch_key: None,
        lease_id,
        command: command.iter().map(|part| part.to_string()).collect(),
        cwd: None,
        env: Default::default(),
        timeout_seconds: RunCommandRequest::DEFAULT_TIMEOUT_SECONDS,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: Default::default(),
        max_output_bytes: None,
        redact_values: Default::default(),
        execution: CommandExecution::Attached,
    }
}

/// A detached execution with closed stdin and no process spec.
pub(super) fn detached(stdin: StdinMode) -> CommandExecution {
    CommandExecution::Detached {
        stdin,
        deadline: Default::default(),
        process: None,
    }
}

/// Registers a worker whose profile is the default one after `adjust`.
async fn inject_adjusted_worker(
    registry: &InMemoryWorkerRegistry,
    worker_id: &str,
    protocol: WorkerDispatchProtocol,
    adjust: impl FnOnce(&mut IsolationCapability),
) -> anyhow::Result<()> {
    let mut profile = default_capability_profile();
    adjust(&mut profile.isolation);
    let mut heartbeat = worker_heartbeat(worker_id, ProviderKind::Mock, profile)?;
    heartbeat.dispatch_protocol = protocol;
    registry.inject(heartbeat).await;
    Ok(())
}

fn test_tenant() -> anyhow::Result<TenantId> {
    Ok(TenantId::new("test-tenant")?)
}

fn test_lease() -> LeaseId {
    LeaseId::new("lease-under-test")
}

pub(super) fn stop_request(lease_id: &LeaseId) -> StopCommandRequest {
    StopCommandRequest {
        lease_id: lease_id.clone(),
        execution_id: ExecutionId::new(EXEC_ID),
        grace_seconds: 1,
    }
}

pub(super) fn restart_request(lease_id: &LeaseId) -> RestartCommandRequest {
    RestartCommandRequest {
        lease_id: lease_id.clone(),
        execution_id: ExecutionId::new(EXEC_ID),
        grace_seconds: 1,
    }
}

pub(super) fn read_logs_request(lease_id: &LeaseId, execution_id: &str, after: u64) -> ReadLogsRequest {
    ReadLogsRequest {
        lease_id: lease_id.clone(),
        execution_id: ExecutionId::new(execution_id),
        after_seq: after,
        max_bytes: None,
        wait_ms: None,
    }
}

pub(super) fn write_stdin_request(lease_id: &LeaseId, data: &[u8], eof: bool) -> anyhow::Result<WriteStdinRequest> {
    Ok(WriteStdinRequest::new(
        lease_id.clone(),
        ExecutionId::new(EXEC_ID),
        StdinSeq::new(0),
        StdinChunk::new(data.to_vec())?,
        StdinEof::from(eof),
    )?)
}

pub(super) struct NoRespondersBus;

#[async_trait::async_trait]
impl crate::bus::BrokerBus for NoRespondersBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Err(async_nats::RequestError::from(async_nats::RequestErrorKind::NoResponders).into())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) async fn registry_with_tracking(changes: bool, watch: bool) -> anyhow::Result<Arc<InMemoryWorkerRegistry>> {
    registry_with_isolation(|isolation| {
        isolation.supports_change_tracking = CapabilitySupport::from(changes);
        isolation.supports_file_watch = CapabilitySupport::from(watch);
    })
    .await
}

pub(super) async fn registry_with_isolation(
    adjust: impl FnOnce(&mut IsolationCapability),
) -> anyhow::Result<Arc<InMemoryWorkerRegistry>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_adjusted_worker(&registry, "tracking", WorkerDispatchProtocol::TargetedV1, adjust).await?;
    Ok(registry)
}

pub(super) struct TimedOutBus;

#[async_trait::async_trait]
impl crate::bus::BrokerBus for TimedOutBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Err(async_nats::RequestError::from(async_nats::RequestErrorKind::TimedOut).into())
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

fn started_response() -> RunCommandResponse {
    RunCommandResponse::Started(trogon_sandbox_contract::RunCommandStartedResponse {
        origin: Default::default(),
        execution_id: ExecutionId::new(EXEC_ID),
    })
}

fn completed_response() -> RunCommandResponse {
    RunCommandResponse::Completed(Default::default())
}

fn response_bytes(response: &RunCommandResponse) -> anyhow::Result<Bytes> {
    Ok(Bytes::from(serde_json::to_vec(response)?))
}

pub(super) async fn run_interactive_with_profile_flag(supported: Option<bool>) -> anyhow::Result<RunCommandResponse> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    if let Some(flag) = supported {
        inject_adjusted_worker(&registry, "mock-w1", Default::default(), |isolation| {
            isolation.supports_interactive_stdin = CapabilitySupport::from(flag);
        })
        .await?;
    }
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    let started = response_bytes(&started_response())?;
    let core = make_core_with_registry(started, registry, true)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let mut request = base_run_request(lease_id, &["cat"]);
    request.execution = detached(StdinMode::Interactive);
    Ok(core.handle_run_command(request).await)
}

pub(super) async fn run_redacting_with_profile_flag(
    supported: Option<bool>,
    values: Vec<trogon_sandbox_contract::RedactValue>,
) -> anyhow::Result<RunCommandResponse> {
    // Registry enforcement stays off: redaction must fail closed anyway.
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    if let Some(flag) = supported {
        inject_adjusted_worker(&registry, "mock-w1", Default::default(), |isolation| {
            isolation.supports_output_redaction = CapabilitySupport::from(flag);
        })
        .await?;
    }
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    let completed = response_bytes(&completed_response())?;
    let core = make_core_with_registry(completed, registry, false)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;

    let mut request = base_run_request(lease_id, &["env"]);
    request.redact_values = RedactValues::new(values)?;
    Ok(core.handle_run_command(request).await)
}

pub(super) fn one_secret() -> Vec<trogon_sandbox_contract::RedactValue> {
    vec![trogon_sandbox_contract::RedactValue::new("s3cr3t-token-value")]
}

/// A worker that replies after `reply_after`, behind a transport that gives
/// up after `default_timeout` unless the caller passes its own timeout, the
/// way the NATS client's default request timeout behaves.
pub(super) struct SlowWorkerBus {
    pub(super) reply: Bytes,
    pub(super) reply_after: Duration,
    pub(super) default_timeout: Duration,
    pub(super) timeouts: std::sync::Mutex<Vec<Duration>>,
}

impl SlowWorkerBus {
    pub(super) fn new(reply_after: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            reply: response_bytes(&completed_response())?,
            reply_after,
            default_timeout: Duration::from_secs(10),
            timeouts: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub(super) async fn reply_within(&self, timeout: Duration) -> anyhow::Result<Bytes> {
        tokio::time::timeout(timeout, tokio::time::sleep(self.reply_after))
            .await
            .map_err(|_| anyhow::anyhow!("request timed out"))?;
        Ok(self.reply.clone())
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for SlowWorkerBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        self.reply_within(self.default_timeout).await
    }

    async fn request_with_timeout(&self, _subject: &str, _payload: Bytes, timeout: Duration) -> anyhow::Result<Bytes> {
        locked(&self.timeouts)?.push(timeout);
        self.reply_within(timeout).await
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn foreground_command(lease_id: LeaseId, timeout_seconds: u32) -> RunCommandRequest {
    let mut request = base_run_request(lease_id, &["sleep", "15"]);
    request.timeout_seconds = timeout_seconds;
    request
}

/// Counts run_command dispatches and answers each after `reply_after`.
/// The first `undelivered` requests find no worker listening.
pub(super) struct CountingWorkerBus {
    pub(super) reply: std::sync::Mutex<RunCommandResponse>,
    pub(super) reply_after: Duration,
    pub(super) dispatches: std::sync::atomic::AtomicUsize,
    pub(super) undelivered: std::sync::atomic::AtomicUsize,
}

impl CountingWorkerBus {
    pub(super) fn new(reply: RunCommandResponse, reply_after: Duration) -> Self {
        Self {
            reply: std::sync::Mutex::new(reply),
            reply_after,
            dispatches: std::sync::atomic::AtomicUsize::new(0),
            undelivered: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(super) fn dispatches(&self) -> usize {
        self.dispatches.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for CountingWorkerBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        self.request_with_timeout(subject, payload, Duration::from_secs(10))
            .await
    }

    async fn request_with_timeout(&self, _subject: &str, _payload: Bytes, _timeout: Duration) -> anyhow::Result<Bytes> {
        if self
            .undelivered
            .try_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            return Err(async_nats::RequestError::from(async_nats::RequestErrorKind::NoResponders).into());
        }
        self.dispatches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(self.reply_after).await;
        let reply = locked(&self.reply)?.clone();
        Ok(Bytes::from(serde_json::to_vec(&reply)?))
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) async fn core_with_counting_worker(
    reply: RunCommandResponse,
    reply_after: Duration,
) -> anyhow::Result<(Arc<BrokerCore>, Arc<CountingWorkerBus>, LeaseId)> {
    let bus = Arc::new(CountingWorkerBus::new(reply, reply_after));
    let core = make_core_with_bus(bus.clone())?;
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    Ok((Arc::new(core), bus, lease_id))
}

pub(super) fn keyed_command(lease_id: &LeaseId, key: &str) -> anyhow::Result<RunCommandRequest> {
    let mut request = foreground_command(lease_id.clone(), 60);
    request.launch_key = Some(LaunchKey::new(key)?);
    Ok(request)
}

pub(super) fn completed_with_stdout(stdout: &str) -> RunCommandResponse {
    RunCommandResponse::Completed(trogon_sandbox_contract::CommandOutput {
        stdout: stdout.as_bytes().to_vec(),
        ..Default::default()
    })
}

pub(super) async fn seed_launch(
    core: &BrokerCore,
    lease_id: &LeaseId,
    key: &str,
    state: LaunchState,
    age: Duration,
) -> anyhow::Result<()> {
    let mut request = keyed_command(lease_id, key)?;
    request.max_output_bytes = Some(trogon_sandbox_protocol::effective_max_output_bytes(
        request.max_output_bytes,
        core.max_command_output_bytes,
    ) as u64);
    let launch = CommandLaunch {
        fingerprint: crate::command_launch::fingerprint(&request)?,
        recorded_at: Some(SystemTime::now() - age),
        state,
    };
    core.command_launches
        .finish(lease_id, &LaunchKey::new(key)?, &launch)
        .await?;
    Ok(())
}

/// Launch journal whose finish fails for entries larger than `limit`,
/// like a JetStream bucket refusing a payload over its maximum.
pub(super) struct SizeLimitedLaunchStore {
    pub(super) inner: InMemoryCommandLaunchStore,
    pub(super) limit: usize,
}

#[async_trait::async_trait]
impl CommandLaunchStore for SizeLimitedLaunchStore {
    async fn claim(&self, lease: &LeaseId, key: &LaunchKey, fingerprint: &str) -> Result<Claim, CommandLaunchError> {
        self.inner.claim(lease, key, fingerprint).await
    }

    async fn finish(&self, lease: &LeaseId, key: &LaunchKey, launch: &CommandLaunch) -> Result<(), CommandLaunchError> {
        if serde_json::to_vec(launch)?.len() > self.limit {
            return Err(CommandLaunchError::backend(std::io::Error::other(
                "maximum payload exceeded",
            )));
        }
        self.inner.finish(lease, key, launch).await
    }

    async fn forget(&self, lease: &LeaseId, key: &LaunchKey) -> Result<(), CommandLaunchError> {
        self.inner.forget(lease, key).await
    }

    async fn replace(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        expected: &CommandLaunch,
        next: &CommandLaunch,
    ) -> Result<bool, CommandLaunchError> {
        self.inner.replace(lease, key, expected, next).await
    }

    async fn forget_lease(&self, lease: &LeaseId) -> Result<(), CommandLaunchError> {
        self.inner.forget_lease(lease).await
    }
}

pub(super) async fn core_with_size_limited_journal(
    reply: RunCommandResponse,
    limit: usize,
) -> anyhow::Result<(Arc<BrokerCore>, Arc<CountingWorkerBus>, LeaseId)> {
    let bus = Arc::new(CountingWorkerBus::new(reply, Duration::ZERO));
    let core = make_core_with_bus(bus.clone())?.with_command_launch_store(Arc::new(SizeLimitedLaunchStore {
        inner: InMemoryCommandLaunchStore::new(),
        limit,
    }));
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    Ok((Arc::new(core), bus, lease_id))
}

/// A core holding a running lease whose provider declares the given
/// `(named processes, readiness)` support, or no profile at all for `None`.
pub(super) async fn process_core(support: Option<(bool, bool)>, reply: Bytes) -> anyhow::Result<(BrokerCore, LeaseId)> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    if let Some((named, readiness)) = support {
        inject_adjusted_worker(&registry, "mock-w1", Default::default(), |isolation| {
            isolation.supports_named_processes = CapabilitySupport::from(named);
            isolation.supports_process_readiness = CapabilitySupport::from(readiness);
        })
        .await?;
    }
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    let core = make_core_with_registry(reply, registry, true)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    Ok((core, lease_id))
}

pub(super) fn process_run_request(lease_id: &LeaseId, execution: CommandExecution) -> RunCommandRequest {
    let mut request = base_run_request(lease_id.clone(), &["serve"]);
    request.execution = execution;
    request
}

/// A detached execution running `process`.
pub(super) fn detached_process(process: ProcessSpec) -> CommandExecution {
    CommandExecution::Detached {
        stdin: StdinMode::Closed,
        deadline: Default::default(),
        process: Some(process),
    }
}

pub(super) fn named(name: &str) -> anyhow::Result<ProcessSpec> {
    Ok(ProcessSpec {
        name: Some(trogon_sandbox_contract::ProcessName::new(name)?),
        readiness: None,
    })
}

pub(super) fn probed() -> anyhow::Result<ProcessSpec> {
    Ok(ProcessSpec {
        name: None,
        readiness: Some(ReadinessProbe::TcpPort {
            port: std::num::NonZeroU16::new(3000).context("zero port")?,
        }),
    })
}

pub(super) fn started_reply() -> anyhow::Result<Bytes> {
    response_bytes(&started_response())
}

pub(super) fn probe_reply(exit_code: i32, stdout: &str) -> anyhow::Result<Bytes> {
    response_bytes(&RunCommandResponse::Completed(CommandOutput {
        exit_code,
        stdout: stdout.as_bytes().to_vec(),
        ..Default::default()
    }))
}

pub(super) fn sandbox_ready_request(lease_id: LeaseId, ports: Vec<u16>) -> anyhow::Result<WaitSandboxReadyRequest> {
    Ok(WaitSandboxReadyRequest {
        lease_id,
        ports: ReadyPorts::new(ports)?,
        timeout_seconds: 0,
    })
}

pub(super) async fn forwarded_output_cap(ceiling: usize, requested: Option<u64>) -> anyhow::Result<Option<u64>> {
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    let mut core = make_core(mock_provider_response()?)?.with_max_command_output_bytes(ceiling);
    let bus = Arc::new(CapturingBus::new(response_bytes(&completed_response())?));
    core.bus = bus.clone();
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    let mut request = base_run_request(lease_id, &["true"]);
    request.max_output_bytes = requested;
    core.handle_run_command(request).await;
    let payload = locked(&bus.last_payload)?
        .clone()
        .context("no payload was dispatched")?;
    Ok(serde_json::from_slice::<RunCommandRequest>(&payload)?.max_output_bytes)
}

pub(super) async fn terminal_with_profile_flag(
    supported: bool,
    op: TerminalOp,
    reply: TerminalResponse,
) -> anyhow::Result<TerminalResponse> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_adjusted_worker(&registry, "mock-w1", Default::default(), |isolation| {
        isolation.supports_terminal = CapabilitySupport::from(supported);
    })
    .await?;
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    let core = make_core_with_registry(Bytes::from(serde_json::to_vec(&reply)?), registry, true)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    Ok(core
        .handle_terminal(TerminalRequest {
            lease_id,
            terminal_id: trogon_sandbox_contract::TerminalId::new("t-1")?,
            op,
        })
        .await)
}

pub(super) fn open_op() -> anyhow::Result<TerminalOp> {
    Ok(TerminalOp::Open {
        cols: TerminalDimension::new(80)?,
        rows: TerminalDimension::new(24)?,
        cmd: Vec::new(),
        env: Default::default(),
        cwd: None,
        resume_from: None,
    })
}

pub(super) async fn core_with_isolation_flags(
    reply: Bytes,
    set: impl FnOnce(&mut IsolationCapability),
) -> anyhow::Result<(BrokerCore, LeaseId)> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    inject_adjusted_worker(&registry, "mock-w1", Default::default(), set).await?;
    let tenant = test_tenant()?;
    let lease_id = test_lease();
    let core = make_core_with_registry(reply, registry, true)?;
    core.active_leases
        .put(&running_source_record(&tenant, &lease_id)?)
        .await?;
    Ok((core, lease_id))
}
