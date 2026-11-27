use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_client::{ClientError, LogFollowEvent, TransportError};
use trogon_sandbox_contract::{
    AccountLimits, CancelOperationRequest, CancelOperationResponse, CommandExecution, CommandOutput,
    CreateCancellationSupport, CreateSandboxGroupResponse, CreateSandboxUserResponse, DeleteEntityResponse,
    DeleteNamedSandboxResponse, DeleteSnapshotTagResponse, Digest, DriveId, DriveRecord, DriveStorage, EntityRef,
    ExecutionId, FileOpRequest, FileOpResponse, GetEntityResponse, GetLeaseByNameResponse, GetSandboxDomainResponse,
    GroupMembershipAction, KillCommandResponse, LeaseCreation, LeaseError, LeaseGranted, LeaseId, LeaseName,
    LeaseReleasedReason, LeaseRequest, LeaseStatus, LeaseUsage, ListEntitiesRequest, ListEntitiesResponse,
    ListProcessesResponse, LogChunk, NativeHandleResponse, NativeSnapshotAcceptance, NetworkPolicy,
    OperationCapabilities, OperationId, OperationRecord, PauseLeaseResponse, PollCommandResponse, ProcessInfo,
    ProcessName, ProcessState, ProviderKind, ReadLogsRequest, ReadLogsResponse, ReadyPorts, ReapLeasesRequest,
    ReapLeasesResponse, RestartCommandRequest, RestartCommandResponse, ResumeLeaseResponse, RunCommandRequest,
    SandboxFileWrite, SandboxGroupName, SandboxTags, SandboxUserName, SnapshotDeleteResponse, SnapshotGetResponse,
    SnapshotLeaseResponse, SnapshotTagHistoryResponse, SnapshotTagName, SnapshotTreeResponse, SshAccessResponse,
    StdinChunk, StdinEof, StdinSeq, StopCommandRequest, StopCommandResponse, StopSignal, SuspendLeaseResponse,
    SuspendLevel, SuspendRequestLevel, TenantId, TenantUsage, TerminalError, TerminalId, TerminalOp, TerminalResponse,
    UnixSignal, WaitReadyResponse, WaitSandboxReadyResponse, WakeLeaseResponse, WriteStdinResponse,
};
use trogon_sandbox_file_ops::FsRoot;
use trogon_sandbox_ws_gateway::{
    BackendError, CapabilitiesResponse, CapabilityRegistry, EventSender, RunCommandOutcome, SandboxBackend,
    follow_logs_stream_id,
};

type FileStore = BTreeMap<(String, String), Vec<u8>>;

/// A failure the mock reports for every terminal operation.
#[derive(Clone, Copy)]
pub enum TerminalFault {
    /// The broker did not answer in time.
    Timeout,
    /// The broker refused the lease.
    Denied,
    /// The broker answered with a terminal failure.
    Failed,
}

/// One terminal operation the gateway sent.
pub type TerminalCall = (LeaseId, TerminalId, TerminalOp);

/// One recorded operation call: the verb, the tenant, and the operation id when it names one.
pub type OperationCall = (String, TenantId, Option<OperationId>);

/// Takes the lock whether or not a panicking test poisoned it.
pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scripted backend that keeps files, processes and terminals in memory.
pub struct MockSandboxBackend {
    capabilities: Arc<CapabilityRegistry>,
    evidence: Mutex<Vec<(LeaseId, Value)>>,
    evidence_subscribe_error: Mutex<Option<String>>,
    logs: Mutex<Vec<(LeaseId, ExecutionId, LogChunk)>>,
    files: Mutex<FileStore>,
    file_roots: Mutex<HashMap<String, TempDir>>,
    processes: Mutex<Vec<(LeaseId, ProcessInfo)>>,
    domain_endpoint: Mutex<String>,
    terminal_ops: Mutex<Vec<TerminalCall>>,
    open_terminals: Mutex<HashMap<String, LeaseId>>,
    terminal_subscriptions: Mutex<Vec<String>>,
    terminal_unsupported: AtomicBool,
    terminal_fault: Mutex<Option<TerminalFault>>,
    domain_replies: Mutex<VecDeque<Result<GetSandboxDomainResponse, ClientError>>>,
    terminal_subscribe_error: Mutex<Option<String>>,
    command_barrier: Mutex<VecDeque<(Arc<Notify>, Arc<Notify>)>>,
    operation_supported: AtomicBool,
    operation_calls: Mutex<Vec<OperationCall>>,
    operation_cancel_requests: Mutex<Vec<CancelOperationRequest>>,
    operation_records: Mutex<Vec<OperationRecord>>,
    operation_cancel_response: Mutex<CancelOperationResponse>,
}

impl MockSandboxBackend {
    pub fn new() -> Self {
        Self {
            capabilities: CapabilityRegistry::new(),
            evidence: Mutex::new(Vec::new()),
            evidence_subscribe_error: Mutex::new(None),
            logs: Mutex::new(Vec::new()),
            files: Mutex::new(BTreeMap::new()),
            file_roots: Mutex::new(HashMap::new()),
            processes: Mutex::new(Vec::new()),
            domain_endpoint: Mutex::new("mock-lease.internal:8080".to_string()),
            terminal_ops: Mutex::new(Vec::new()),
            open_terminals: Mutex::new(HashMap::new()),
            terminal_subscriptions: Mutex::new(Vec::new()),
            terminal_unsupported: AtomicBool::new(false),
            terminal_fault: Mutex::new(None),
            domain_replies: Mutex::new(VecDeque::new()),
            terminal_subscribe_error: Mutex::new(None),
            command_barrier: Mutex::new(VecDeque::new()),
            operation_supported: AtomicBool::new(false),
            operation_calls: Mutex::new(Vec::new()),
            operation_cancel_requests: Mutex::new(Vec::new()),
            operation_records: Mutex::new(Vec::new()),
            operation_cancel_response: Mutex::new(CancelOperationResponse::Unsupported),
        }
    }

    pub fn set_domain_endpoint(&self, endpoint: &str) {
        *locked(&self.domain_endpoint) = endpoint.to_string();
    }

    pub fn set_operation_support(&self, supported: bool) {
        self.operation_supported.store(supported, Ordering::SeqCst);
    }

    pub fn set_operation_cancel_response(&self, response: CancelOperationResponse) {
        *locked(&self.operation_cancel_response) = response;
    }

    pub fn record_operation(&self, operation: OperationRecord) {
        locked(&self.operation_records).push(operation);
    }

    pub fn operation_calls(&self) -> Vec<OperationCall> {
        locked(&self.operation_calls).clone()
    }

    pub fn operation_cancel_requests(&self) -> Vec<CancelOperationRequest> {
        locked(&self.operation_cancel_requests).clone()
    }

    pub fn set_terminal_unsupported(&self, unsupported: bool) {
        self.terminal_unsupported.store(unsupported, Ordering::SeqCst);
    }

    /// Replies the next domain lookups return, in order. When none is queued the lookup answers with the endpoint
    /// from [`Self::set_domain_endpoint`].
    pub fn queue_domain_reply(&self, reply: Result<GetSandboxDomainResponse, ClientError>) {
        locked(&self.domain_replies).push_back(reply);
    }

    pub fn set_terminal_fault(&self, fault: Option<TerminalFault>) {
        *locked(&self.terminal_fault) = fault;
    }

    pub fn fail_terminal_subscription(&self, message: &str) {
        *locked(&self.terminal_subscribe_error) = Some(message.to_string());
    }

    pub fn terminal_ops(&self) -> Vec<TerminalCall> {
        locked(&self.terminal_ops).clone()
    }

    pub fn terminal_subscriptions(&self) -> Vec<String> {
        locked(&self.terminal_subscriptions).clone()
    }

    pub fn fail_evidence_subscription(&self, message: &str) {
        *locked(&self.evidence_subscribe_error) = Some(message.to_string());
    }

    pub fn queue_log_chunk(&self, lease_id: LeaseId, execution_id: &str, chunk: LogChunk) {
        locked(&self.logs).push((lease_id, ExecutionId::new(execution_id), chunk));
    }

    pub fn queue_evidence(&self, lease_id: LeaseId, event: Value) {
        locked(&self.evidence).push((lease_id, event));
    }

    /// Makes the next `run_command` wait: the first handle fires when it has entered, the second releases it.
    pub fn pause_next_command(&self) -> (Arc<Notify>, Arc<Notify>) {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        locked(&self.command_barrier).push_back((entered.clone(), release.clone()));
        (entered, release)
    }

    pub fn capabilities(&self) -> Arc<CapabilityRegistry> {
        Arc::clone(&self.capabilities)
    }

    fn forget_lease(&self, lease_id: &LeaseId) {
        locked(&self.file_roots).remove(lease_id.as_str());
        locked(&self.files).retain(|(id, _), _| id != lease_id.as_str());
    }

    fn file_root(&self, lease_id: &LeaseId) -> Result<FsRoot, Box<ClientError>> {
        let mut roots = locked(&self.file_roots);
        if !roots.contains_key(lease_id.as_str()) {
            roots.insert(
                lease_id.as_str().to_string(),
                TempDir::new().map_err(|error| Box::new(transport(error)))?,
            );
        }
        let path = roots
            .get(lease_id.as_str())
            .map(|dir| dir.path().to_path_buf())
            .unwrap_or_default();
        FsRoot::new(path, None).map_err(|error| Box::new(transport(error)))
    }
}

fn transport(error: impl std::fmt::Display) -> ClientError {
    ClientError::Transport(TransportError::InvalidRequest(error.to_string()))
}

impl Default for MockSandboxBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn granted(lease_id: &str, provider: ProviderKind, sandbox: &str) -> LeaseGranted {
    LeaseGranted {
        lease_id: LeaseId::new(lease_id),
        provider,
        sandbox_id: sandbox.to_string(),
        evidence_subject: format!("sandbox.evidence.{lease_id}.>"),
        deadline: SystemTime::now() + Duration::from_secs(300),
        created: LeaseCreation::Created,
        capabilities: Vec::new(),
        entity_id: None,
        operation_id: None,
    }
}

#[async_trait]
impl SandboxBackend for MockSandboxBackend {
    async fn operation_capabilities(
        &self,
        _api_key: &str,
        tenant: TenantId,
    ) -> Result<OperationCapabilities, ClientError> {
        locked(&self.operation_calls).push(("capabilities".into(), tenant, None));
        Ok(OperationCapabilities {
            supports_create_cancellation: CreateCancellationSupport::from(
                self.operation_supported.load(Ordering::SeqCst),
            ),
        })
    }

    async fn cancel_operation(
        &self,
        _api_key: &str,
        request: CancelOperationRequest,
    ) -> Result<CancelOperationResponse, ClientError> {
        locked(&self.operation_cancel_requests).push(request.clone());
        locked(&self.operation_calls).push(("cancel".into(), request.tenant, Some(request.operation_id)));
        Ok(locked(&self.operation_cancel_response).clone())
    }

    async fn get_operation(
        &self,
        _api_key: &str,
        tenant: TenantId,
        operation_id: &OperationId,
    ) -> Result<Option<OperationRecord>, ClientError> {
        locked(&self.operation_calls).push(("get".into(), tenant.clone(), Some(operation_id.clone())));
        Ok(locked(&self.operation_records)
            .iter()
            .find(|record| record.tenant == tenant && &record.id == operation_id)
            .cloned())
    }

    async fn lease(
        &self,
        _api_key: &str,
        request: &LeaseRequest,
        _timeout: Duration,
    ) -> Result<LeaseGranted, ClientError> {
        let provider = request.provider_pref.first().copied().unwrap_or(ProviderKind::Mock);
        Ok(granted("mock-lease", provider, "mock-sandbox"))
    }

    async fn cancel(&self, _api_key: &str, lease_id: &LeaseId) -> Result<(), ClientError> {
        self.forget_lease(lease_id);
        Ok(())
    }

    async fn reap_leases(&self, _api_key: &str, request: ReapLeasesRequest) -> Result<ReapLeasesResponse, ClientError> {
        Ok(ReapLeasesResponse::Reaped {
            dry_run: request.dry_run(),
            matched: vec![],
            released: vec![],
            failed: vec![],
        })
    }

    async fn update_tags(&self, _api_key: &str, _lease_id: &LeaseId, _tags: SandboxTags) -> Result<(), ClientError> {
        Ok(())
    }

    async fn wait_released(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _timeout: Duration,
    ) -> Result<LeaseReleasedReason, ClientError> {
        Ok(LeaseReleasedReason::Completed)
    }

    async fn stop_lease(&self, _api_key: &str, _tenant: TenantId, _lease_id: &LeaseId) -> Result<(), ClientError> {
        Ok(())
    }

    async fn pause_lease(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _lease_id: &LeaseId,
        _max_paused_seconds: Option<u32>,
    ) -> Result<PauseLeaseResponse, ClientError> {
        Ok(PauseLeaseResponse::Unsupported)
    }

    async fn suspend_lease(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        lease_id: &LeaseId,
        level: SuspendRequestLevel,
        _max_suspended_seconds: Option<u32>,
    ) -> Result<SuspendLeaseResponse, ClientError> {
        if matches!(level, SuspendRequestLevel::Disk) {
            return Ok(SuspendLeaseResponse::Suspended {
                level: SuspendLevel::Disk,
                reap_at: SystemTime::UNIX_EPOCH,
                operation_id: None,
            });
        }
        Err(ClientError::Denied(LeaseError::SuspendUnsupported {
            lease_id: lease_id.clone(),
            requested: level,
        }))
    }

    async fn wake_lease(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<WakeLeaseResponse, ClientError> {
        Err(ClientError::Denied(LeaseError::LeaseNotRunning {
            lease_id: lease_id.clone(),
        }))
    }

    async fn snapshot_lease(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _lease_id: &LeaseId,
        _accept_native: NativeSnapshotAcceptance,
    ) -> Result<SnapshotLeaseResponse, ClientError> {
        Ok(SnapshotLeaseResponse::Unsupported)
    }

    async fn snapshot_delete(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _digest: Digest,
    ) -> Result<SnapshotDeleteResponse, ClientError> {
        Ok(SnapshotDeleteResponse::NotFound)
    }

    async fn resume_lease(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _lease_id: &LeaseId,
    ) -> Result<ResumeLeaseResponse, ClientError> {
        Ok(ResumeLeaseResponse::Unsupported)
    }

    async fn lease_usage(
        &self,
        _api_key: &str,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<LeaseUsage, ClientError> {
        let now = SystemTime::now();
        Ok(LeaseUsage {
            lease_id: lease_id.clone(),
            tenant,
            status: LeaseStatus::Running,
            created_at: now,
            ended_at: None,
            last_activity_at: now,
            commands_run: 0,
            command_output_bytes: 0,
            file_bytes_in: 0,
            file_bytes_out: 0,
            running_seconds: 0,
            paused_seconds: 0,
            suspended_seconds: 0,
            suspend_artifact_bytes: None,
            gauges: None,
        })
    }

    async fn tenant_usage(&self, _api_key: &str, _tenant: TenantId) -> Result<TenantUsage, ClientError> {
        Ok(TenantUsage::default())
    }

    async fn account_limits(&self, _api_key: &str, _tenant: TenantId) -> Result<AccountLimits, ClientError> {
        Ok(AccountLimits {
            active_leases: 0,
            max_active_leases: 50,
            cpu_seconds_used: 0,
            cpu_seconds_cap: 3600,
            wall_seconds_used: 0,
            wall_seconds_cap: 7200,
            max_cpu_seconds_per_request: 3600,
            max_memory_mib_per_request: 8192,
            per_tag: None,
        })
    }

    async fn delete_lease(&self, _api_key: &str, _tenant: TenantId, lease_id: &LeaseId) -> Result<(), ClientError> {
        self.forget_lease(lease_id);
        Ok(())
    }

    async fn extend_lease_timeout(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        additional_seconds: u32,
    ) -> Result<SystemTime, ClientError> {
        Ok(SystemTime::now() + Duration::from_secs(u64::from(additional_seconds)))
    }

    async fn fork_lease(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _source_lease_id: &LeaseId,
        _ttl_seconds: u32,
        _idempotency_key: String,
    ) -> Result<LeaseGranted, ClientError> {
        Ok(granted("mock-forked-lease", ProviderKind::Mock, "mock-forked-sandbox"))
    }

    async fn update_network_policy(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _network_policy: NetworkPolicy,
    ) -> Result<(), ClientError> {
        Ok(())
    }

    async fn create_drive(
        &self,
        _api_key: &str,
        tenant: TenantId,
        drive_id: DriveId,
        size_gib: Option<u64>,
        storage: DriveStorage,
    ) -> Result<DriveRecord, ClientError> {
        Ok(DriveRecord {
            id: drive_id,
            tenant,
            size_gib: size_gib.unwrap_or(0),
            created_at: SystemTime::now(),
            attached_lease: None,
            storage,
        })
    }

    async fn list_drives(&self, _api_key: &str, _tenant: TenantId) -> Result<Vec<DriveRecord>, ClientError> {
        Ok(vec![])
    }

    async fn delete_drive(&self, _api_key: &str, _tenant: TenantId, _drive_id: DriveId) -> Result<(), ClientError> {
        Ok(())
    }

    async fn run_command(&self, _api_key: &str, request: RunCommandRequest) -> Result<RunCommandOutcome, ClientError> {
        let barrier = locked(&self.command_barrier).pop_front();
        if let Some((entered, release)) = barrier {
            entered.notify_one();
            release.notified().await;
        }
        if request.command == ["true"] {
            return Ok(RunCommandOutcome::Completed(CommandOutput {
                exit_code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                timed_out: Default::default(),
                truncated: Default::default(),
            }));
        }
        let CommandExecution::Detached { process, .. } = request.execution else {
            return Err(ClientError::Denied(LeaseError::Backend {
                message: "mock backend cannot execute foreground commands".into(),
            }));
        };
        let started = |execution_id| RunCommandOutcome::Started {
            execution_id,
            origin: Default::default(),
        };
        let Some(spec) = process else {
            return Ok(started(ExecutionId::new("mock-execution")));
        };
        let mut processes = locked(&self.processes);
        let running = processes.iter().find(|(lease, info)| {
            lease == &request.lease_id
                && info.name == spec.name
                && spec.name.is_some()
                && info.state == ProcessState::Running
        });
        if let Some((_, info)) = running {
            return Ok(started(info.execution_id.clone()));
        }
        let execution_id = ExecutionId::new(format!("mock-process-{}", processes.len() + 1));
        processes.push((
            request.lease_id,
            ProcessInfo {
                execution_id: execution_id.clone(),
                name: spec.name,
                readiness: spec.readiness,
                state: ProcessState::Running,
            },
        ));
        Ok(started(execution_id))
    }

    async fn list_processes(&self, _api_key: &str, lease_id: &LeaseId) -> Result<ListProcessesResponse, ClientError> {
        let processes = locked(&self.processes)
            .iter()
            .filter(|(lease, _)| lease == lease_id)
            .map(|(_, info)| info.clone())
            .collect();
        Ok(ListProcessesResponse::Processes { processes })
    }

    async fn wait_ready(
        &self,
        _api_key: &str,
        lease_id: &LeaseId,
        execution_id: &ExecutionId,
        _timeout_seconds: u32,
    ) -> Result<WaitReadyResponse, ClientError> {
        let processes = locked(&self.processes);
        let found = processes
            .iter()
            .find(|(lease, info)| lease == lease_id && &info.execution_id == execution_id);
        Ok(match found {
            None => WaitReadyResponse::NotFound,
            Some((_, info)) if info.readiness.is_none() => WaitReadyResponse::NoProbe,
            Some((_, info)) => match info.state {
                ProcessState::Running => WaitReadyResponse::Ready,
                ProcessState::Exited { exit_code } => WaitReadyResponse::Failed { exit_code },
            },
        })
    }

    async fn wait_sandbox_ready(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _ports: ReadyPorts,
        _timeout_seconds: u32,
    ) -> Result<WaitSandboxReadyResponse, ClientError> {
        Ok(WaitSandboxReadyResponse::Ready)
    }

    async fn kill_process(
        &self,
        _api_key: &str,
        lease_id: &LeaseId,
        name: &ProcessName,
        _signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError> {
        let mut processes = locked(&self.processes);
        let running = processes.iter_mut().find(|(lease, info)| {
            lease == lease_id && info.name.as_ref() == Some(name) && info.state == ProcessState::Running
        });
        Ok(match running {
            Some((_, info)) => {
                info.state = ProcessState::Exited { exit_code: 143 };
                KillCommandResponse::Signaled
            }
            None => KillCommandResponse::NotFound,
        })
    }

    async fn poll_command(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _execution_id: &ExecutionId,
    ) -> Result<PollCommandResponse, ClientError> {
        Ok(PollCommandResponse::NotFound)
    }

    async fn kill_command(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _execution_id: &ExecutionId,
        _signal: UnixSignal,
    ) -> Result<KillCommandResponse, ClientError> {
        Ok(KillCommandResponse::Signaled)
    }

    async fn stop_command(
        &self,
        _api_key: &str,
        _request: StopCommandRequest,
    ) -> Result<StopCommandResponse, ClientError> {
        Ok(StopCommandResponse::Stopped {
            signal: StopSignal::Term,
        })
    }

    async fn restart_command(
        &self,
        _api_key: &str,
        _request: RestartCommandRequest,
    ) -> Result<RestartCommandResponse, ClientError> {
        Ok(RestartCommandResponse::Unsupported)
    }

    async fn read_logs(&self, _api_key: &str, _request: ReadLogsRequest) -> Result<ReadLogsResponse, ClientError> {
        Ok(ReadLogsResponse::Unsupported)
    }

    async fn write_stdin(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _execution_id: &ExecutionId,
        seq: StdinSeq,
        _data: StdinChunk,
        _eof: StdinEof,
    ) -> Result<WriteStdinResponse, ClientError> {
        Ok(WriteStdinResponse::Written {
            next_seq: StdinSeq::new(seq.get() + 1),
        })
    }

    async fn terminal(
        &self,
        _api_key: &str,
        lease_id: &LeaseId,
        terminal_id: TerminalId,
        op: TerminalOp,
    ) -> Result<TerminalResponse, ClientError> {
        locked(&self.terminal_ops).push((lease_id.clone(), terminal_id.clone(), op.clone()));
        if self.terminal_unsupported.load(Ordering::SeqCst) {
            return Ok(TerminalResponse::Unsupported);
        }
        let fault = *locked(&self.terminal_fault);
        match fault {
            Some(TerminalFault::Timeout) => return Err(ClientError::Timeout(Duration::from_secs(1))),
            Some(TerminalFault::Denied) => {
                return Ok(TerminalResponse::Denied(LeaseError::LeaseNotRunning {
                    lease_id: lease_id.clone(),
                }));
            }
            Some(TerminalFault::Failed) => return Ok(TerminalResponse::Failed(TerminalError::NotFound)),
            None => {}
        }
        let mut open = locked(&self.open_terminals);
        Ok(match op {
            TerminalOp::Open { .. } => {
                open.insert(terminal_id.as_str().to_string(), lease_id.clone());
                TerminalResponse::Opened { terminal_id }
            }
            _ => match open.get(terminal_id.as_str()) {
                None => TerminalResponse::Failed(TerminalError::NotFound),
                Some(owner) if owner != lease_id => TerminalResponse::Failed(TerminalError::IdInUse),
                Some(_) => {
                    if op == TerminalOp::Close {
                        open.remove(terminal_id.as_str());
                    }
                    TerminalResponse::Done
                }
            },
        })
    }

    async fn subscribe_terminal(
        &self,
        terminal_id: TerminalId,
        tx: EventSender,
        _stop: CancellationToken,
    ) -> Result<(), BackendError> {
        if let Some(message) = locked(&self.terminal_subscribe_error).clone() {
            return Err(BackendError::Subscribe(Box::new(transport(message))));
        }
        locked(&self.terminal_subscriptions).push(terminal_id.as_str().to_string());
        let greeting = json!({"type": "data", "seq": 0, "data": "JCA="});
        let _ = tx.send((terminal_id.as_str().to_string(), greeting)).await;
        Ok(())
    }

    async fn write_files(
        &self,
        _api_key: &str,
        lease_id: &LeaseId,
        files: Vec<SandboxFileWrite>,
        _as_user: Option<SandboxUserName>,
    ) -> Result<(), ClientError> {
        let mut store = locked(&self.files);
        for file in files {
            store.insert((lease_id.as_str().to_string(), file.path), file.content);
        }
        Ok(())
    }

    async fn read_file(
        &self,
        _api_key: &str,
        lease_id: &LeaseId,
        path: String,
        _as_user: Option<SandboxUserName>,
    ) -> Result<Vec<u8>, ClientError> {
        if path == "/unreadable" {
            return Err(ClientError::Denied(LeaseError::Backend {
                message: "unreadable".into(),
            }));
        }
        locked(&self.files)
            .get(&(lease_id.as_str().to_string(), path.clone()))
            .cloned()
            .ok_or(ClientError::FileNotFound(path))
    }

    async fn file_op(&self, _api_key: &str, request: FileOpRequest) -> Result<FileOpResponse, ClientError> {
        let root = self.file_root(&request.lease_id).map_err(|error| *error)?;
        let response = tokio::task::spawn_blocking(move || root.execute(&request.op))
            .await
            .map_err(transport)?;
        match response {
            FileOpResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    async fn create_user(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        username: SandboxUserName,
    ) -> Result<CreateSandboxUserResponse, ClientError> {
        Ok(CreateSandboxUserResponse::Created {
            home_dir: format!("/home/{}", username.as_str()),
        })
    }

    async fn create_group(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        groupname: SandboxGroupName,
    ) -> Result<CreateSandboxGroupResponse, ClientError> {
        Ok(CreateSandboxGroupResponse::Created {
            shared_dir: format!("/srv/{}", groupname.as_str()),
        })
    }

    async fn modify_group_membership(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _username: SandboxUserName,
        _groupname: SandboxGroupName,
        _action: GroupMembershipAction,
    ) -> Result<(), ClientError> {
        Ok(())
    }

    async fn get_sandbox_domain(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _port: u16,
    ) -> Result<GetSandboxDomainResponse, ClientError> {
        if let Some(reply) = locked(&self.domain_replies).pop_front() {
            return reply;
        }
        Ok(GetSandboxDomainResponse::Available {
            endpoint: locked(&self.domain_endpoint).clone(),
            url: None,
            expires_at: None,
            access: None,
        })
    }

    async fn native_handle(&self, _api_key: &str, _lease_id: &LeaseId) -> Result<NativeHandleResponse, ClientError> {
        Ok(NativeHandleResponse::Unsupported)
    }

    async fn ssh_access(
        &self,
        _api_key: &str,
        _lease_id: &LeaseId,
        _ttl_seconds: Option<u32>,
    ) -> Result<SshAccessResponse, ClientError> {
        Ok(SshAccessResponse::Unsupported)
    }

    async fn get_entity(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _entity: EntityRef,
    ) -> Result<GetEntityResponse, ClientError> {
        Ok(GetEntityResponse::NotFound)
    }

    async fn list_entities(
        &self,
        _api_key: &str,
        _request: ListEntitiesRequest,
    ) -> Result<ListEntitiesResponse, ClientError> {
        Ok(ListEntitiesResponse::Listed {
            rows: Vec::new(),
            next_cursor: None,
        })
    }

    async fn delete_entity(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _entity: EntityRef,
    ) -> Result<DeleteEntityResponse, ClientError> {
        Ok(DeleteEntityResponse::NotFound)
    }

    async fn get_lease_by_name(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _name: LeaseName,
    ) -> Result<GetLeaseByNameResponse, ClientError> {
        Ok(GetLeaseByNameResponse::NotFound)
    }

    async fn delete_named_sandbox(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _name: LeaseName,
    ) -> Result<DeleteNamedSandboxResponse, ClientError> {
        Ok(DeleteNamedSandboxResponse::NotFound)
    }

    async fn snapshot_get(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _digest: Digest,
    ) -> Result<SnapshotGetResponse, ClientError> {
        Ok(SnapshotGetResponse::NotFound)
    }

    async fn snapshot_tree(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _digest: Digest,
    ) -> Result<SnapshotTreeResponse, ClientError> {
        Ok(SnapshotTreeResponse {
            denied: None,
            nodes: vec![],
        })
    }

    async fn set_snapshot_tag(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _name: SnapshotTagName,
        _digest: Digest,
    ) -> Result<(), ClientError> {
        Ok(())
    }

    async fn delete_snapshot_tag(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _name: SnapshotTagName,
    ) -> Result<DeleteSnapshotTagResponse, ClientError> {
        Ok(DeleteSnapshotTagResponse::Applied)
    }

    async fn snapshot_tag_history(
        &self,
        _api_key: &str,
        _tenant: TenantId,
        _name: SnapshotTagName,
    ) -> Result<SnapshotTagHistoryResponse, ClientError> {
        Ok(SnapshotTagHistoryResponse {
            denied: None,
            entries: vec![],
        })
    }

    async fn list_capabilities(&self) -> CapabilitiesResponse {
        self.capabilities.snapshot().await
    }

    async fn follow_logs(
        &self,
        _api_key: String,
        request: ReadLogsRequest,
        tx: EventSender,
    ) -> Result<(), BackendError> {
        let stream_id = follow_logs_stream_id(&request.lease_id, &request.execution_id);
        let chunks: Vec<LogChunk> = locked(&self.logs)
            .iter()
            .filter(|(lease, exec, chunk)| {
                *lease == request.lease_id && *exec == request.execution_id && chunk.log_seq > request.after_seq
            })
            .map(|(_, _, chunk)| chunk.clone())
            .collect();
        let mut next_seq = request.after_seq;
        for chunk in chunks {
            next_seq = chunk.log_seq;
            let _ = tx
                .send((stream_id.clone(), log_event(&LogFollowEvent::Chunk(chunk))))
                .await;
        }
        let end = LogFollowEvent::End {
            next_seq,
            truncated: false,
        };
        let _ = tx.send((stream_id, log_event(&end))).await;
        Ok(())
    }

    async fn subscribe_evidence(
        &self,
        _api_key: String,
        lease_id: LeaseId,
        tx: EventSender,
    ) -> Result<(), BackendError> {
        if let Some(message) = locked(&self.evidence_subscribe_error).clone() {
            return Err(BackendError::Subscribe(Box::new(transport(message))));
        }
        let queued: Vec<Value> = locked(&self.evidence)
            .iter()
            .filter(|(id, _)| *id == lease_id)
            .map(|(_, event)| event.clone())
            .collect();
        for event in queued {
            let _ = tx.send((lease_id.as_str().to_string(), event)).await;
        }
        Ok(())
    }
}

fn log_event(event: &LogFollowEvent) -> Value {
    match event {
        LogFollowEvent::Chunk(chunk) => json!({
            "type": "chunk",
            "log_seq": chunk.log_seq,
            "stream": chunk.stream,
            "seq": chunk.seq,
            "data": chunk.data,
        }),
        LogFollowEvent::End { next_seq, truncated } => {
            json!({"type": "end", "next_seq": next_seq, "truncated": truncated})
        }
        _ => json!({"type": "other"}),
    }
}
