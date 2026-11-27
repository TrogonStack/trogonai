//! Shared fixtures: a stateful fake of the Modal API, in-memory volume stores, a throwaway NATS server, and
//! request builders.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_nats::Client;
use async_trait::async_trait;
use object_store::ObjectStore;
use object_store::memory::InMemory;
use serde_json::{Value, json};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_contract::{
    CredentialScope, LeaseId, LeaseRequest, NativeSnapshotId, PullSecretId, SandboxId, TenantId, WorkerId,
};
use trogon_sandbox_modal_worker::{
    ExecResult, ModalApiError, ModalAppName, ModalConfig, ModalProvider, ModalSandbox, ModalSandboxApi,
    ModalSandboxSpec, ModalTunnel, ModalWorker, ModalWorkerError, VolumeSettings, VolumeStores, WorkspaceError,
};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};
use trogon_sandbox_volumes::VolumeRegion;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type Boxed<T> = Result<T, Box<dyn std::error::Error>>;

/// How a fake call fails.
#[derive(Clone, Copy)]
pub enum Failure {
    NotFound,
    Other,
}

impl Failure {
    fn error(self, what: &str) -> ModalApiError {
        match self {
            Self::NotFound => ModalApiError::SandboxNotFound(what.to_owned()),
            Self::Other => ModalApiError::Modal {
                kind: "Boom".to_owned(),
                message: what.to_owned(),
            },
        }
    }
}

/// The fake's data and the failures it can be told to produce.
#[derive(Default)]
pub struct State {
    pub created: Vec<ModalSandboxSpec>,
    pub sandboxes: HashMap<String, String>,
    pub writes: Vec<(String, String, Vec<u8>)>,
    pub terminated: Vec<String>,
    pub next: usize,
    pub create_failure: Option<Failure>,
    pub get_failure: Option<Failure>,
    pub write_failure: Option<Failure>,
    pub terminate_failure: Option<Failure>,
    pub tunnels_failure: Option<Failure>,
    pub snapshot_failure: Option<Failure>,
    pub never_running: bool,
    pub polls_before_running: usize,
    pub tunnels: Vec<ModalTunnel>,
    pub snapshots: usize,
}

pub struct FakeModal(Mutex<State>);

fn guard<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeModal {
    pub fn new() -> Self {
        Self(Mutex::new(State::default()))
    }

    pub fn with<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        change(&mut guard(&self.0))
    }

    /// A sandbox Modal knows that no lease of this worker holds.
    pub fn add_sandbox(&self, id: &str) {
        self.with(|state| state.sandboxes.insert(id.to_owned(), "running".to_owned()));
    }
}

pub fn sandbox_id(value: &str) -> Boxed<SandboxId> {
    Ok(SandboxId::new(value)?)
}

pub fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(value)
}

#[async_trait]
impl ModalSandboxApi for FakeModal {
    async fn create_sandbox(&self, spec: &ModalSandboxSpec) -> Result<ModalSandbox, ModalApiError> {
        let mut state = guard(&self.0);
        if let Some(failure) = state.create_failure {
            return Err(failure.error("create"));
        }
        state.next += 1;
        let id = format!("sb-{}", state.next);
        state.created.push(spec.clone());
        let initial = if state.never_running || state.polls_before_running > 0 {
            "created"
        } else {
            "running"
        };
        state.sandboxes.insert(id.clone(), initial.to_owned());
        Ok(ModalSandbox {
            id: SandboxId::new(id).map_err(|_| ModalApiError::NoOutput)?,
            state: initial.to_owned(),
            exit_code: None,
        })
    }

    async fn get_sandbox(&self, sandbox_id: &SandboxId) -> Result<ModalSandbox, ModalApiError> {
        let mut state = guard(&self.0);
        if let Some(failure) = state.get_failure {
            return Err(failure.error("get"));
        }
        let never_running = state.never_running;
        let polls = state.polls_before_running;
        let Some(current) = state.sandboxes.get(sandbox_id.as_str()).cloned() else {
            return Err(ModalApiError::SandboxNotFound(sandbox_id.as_str().to_owned()));
        };
        let reported = if never_running {
            current
        } else if polls > 0 {
            state.polls_before_running -= 1;
            current
        } else {
            "running".to_owned()
        };
        state.sandboxes.insert(sandbox_id.as_str().to_owned(), reported.clone());
        Ok(ModalSandbox {
            id: sandbox_id.clone(),
            state: reported,
            exit_code: None,
        })
    }

    async fn exec(&self, _sandbox_id: &SandboxId, _command: &[String]) -> Result<ExecResult, ModalApiError> {
        Ok(ExecResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    async fn write_file(&self, sandbox_id: &SandboxId, remote_path: &str, data: &[u8]) -> Result<(), ModalApiError> {
        let mut state = guard(&self.0);
        if let Some(failure) = state.write_failure {
            return Err(failure.error("write"));
        }
        state
            .writes
            .push((sandbox_id.as_str().to_owned(), remote_path.to_owned(), data.to_vec()));
        Ok(())
    }

    async fn terminate(&self, sandbox_id: &SandboxId) -> Result<(), ModalApiError> {
        let mut state = guard(&self.0);
        state.terminated.push(sandbox_id.as_str().to_owned());
        if let Some(failure) = state.terminate_failure {
            return Err(failure.error("terminate"));
        }
        state.sandboxes.remove(sandbox_id.as_str());
        Ok(())
    }

    async fn tunnels(&self, _sandbox_id: &SandboxId) -> Result<Vec<ModalTunnel>, ModalApiError> {
        let state = guard(&self.0);
        if let Some(failure) = state.tunnels_failure {
            return Err(failure.error("tunnels"));
        }
        Ok(state.tunnels.clone())
    }

    async fn snapshot_filesystem(
        &self,
        _sandbox_id: &SandboxId,
        _ttl_seconds: u64,
    ) -> Result<NativeSnapshotId, ModalApiError> {
        let mut state = guard(&self.0);
        if let Some(failure) = state.snapshot_failure {
            return Err(failure.error("snapshot"));
        }
        state.snapshots += 1;
        NativeSnapshotId::new(format!("im-{}", state.snapshots)).map_err(|_| ModalApiError::NoOutput)
    }
}

/// Volume stores backed by memory that remember which scopes were opened.
pub struct Stores {
    pub store: Arc<InMemory>,
    pub opened: Mutex<Vec<CredentialScope>>,
    pub fail: AtomicBool,
}

impl Stores {
    pub fn new() -> Self {
        Self {
            store: Arc::new(InMemory::new()),
            opened: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        }
    }

    pub fn opened(&self) -> Vec<CredentialScope> {
        guard(&self.opened).clone()
    }
}

#[async_trait]
impl VolumeStores for Stores {
    async fn open(&self, _tenant: &TenantId, scope: &CredentialScope) -> Result<Arc<dyn ObjectStore>, WorkspaceError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(WorkspaceError::CredentialToken);
        }
        guard(&self.opened).push(scope.clone());
        Ok(self.store.clone())
    }
}

pub fn volume_settings() -> Boxed<VolumeSettings> {
    Ok(VolumeSettings {
        region: VolumeRegion::new("us-east-1")?,
        endpoint: None,
        credential_id: PullSecretId::new("modal-volume"),
    })
}

pub fn config() -> Boxed<ModalConfig> {
    Ok(ModalConfig {
        app_name: ModalAppName::new("test-app")?,
        environment: None,
        default_cpu_cores: None,
        max_vcpus: 4,
        regions: Vec::new(),
        max_concurrent_leases: 3,
        volume: volume_settings()?,
    })
}

/// A fake API, fake stores and a provider driving the one with the other.
pub struct Env {
    pub api: Arc<FakeModal>,
    pub stores: Arc<Stores>,
    pub provider: Arc<ModalProvider>,
}

impl Env {
    pub fn start() -> Boxed<Self> {
        Self::with_config(|_| {})
    }

    pub fn with_config(change: impl FnOnce(&mut ModalConfig)) -> Boxed<Self> {
        let api = Arc::new(FakeModal::new());
        let stores = Arc::new(Stores::new());
        let mut settings = config()?;
        change(&mut settings);
        let provider = Arc::new(ModalProvider::new(api.clone(), stores.clone(), settings));
        Ok(Self { api, stores, provider })
    }
}

pub fn request_json() -> Value {
    json!({
        "tenant": "test",
        "provider_pref": ["modal"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": {"registry": "docker.io", "repository": "library/ubuntu", "tag": "24.04"},
            },
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {"KEY": "value"},
        },
        "ttl_seconds": 60,
        "idempotency_key": "test",
        "persistent": false,
    })
}

pub fn request() -> Boxed<LeaseRequest> {
    Ok(serde_json::from_value(request_json())?)
}

/// A request changed through `change` before it is parsed.
pub fn request_with(change: impl FnOnce(&mut Value)) -> Boxed<LeaseRequest> {
    let mut wire = request_json();
    change(&mut wire);
    Ok(serde_json::from_value(wire)?)
}

/// A throwaway NATS server.
pub struct Nats {
    inner: CoreTestServer,
}

impl Nats {
    pub async fn start() -> Self {
        Self {
            inner: CoreTestServer::start().await,
        }
    }

    pub fn url(&self) -> String {
        format!("nats://{}", self.inner.address())
    }

    pub async fn client(&self) -> Boxed<Client> {
        Ok(async_nats::connect(self.url()).await?)
    }
}

/// A worker serving a provider on a NATS server until stopped.
pub struct Running {
    pub env: Env,
    pub nats: Nats,
    pub client: Client,
    pub worker_id: WorkerId,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), ModalWorkerError>>>,
}

impl Running {
    pub async fn start() -> Boxed<Self> {
        Self::start_with(Env::start()?, None).await
    }

    pub async fn start_with(env: Env, drain: Option<Duration>) -> Boxed<Self> {
        Self::start_on(Nats::start().await, env, drain).await
    }

    pub async fn start_on(nats: Nats, env: Env, drain: Option<Duration>) -> Boxed<Self> {
        let client = nats.client().await?;
        let worker_id = WorkerId::new("worker-1")?;
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let mut worker = ModalWorker::new(
            client.clone(),
            env.provider.clone(),
            worker_id.clone(),
            AuthorityConfig::new(AuthorityMode::Off),
        );
        if let Some(drain) = drain {
            worker = worker.with_drain_timeout(drain);
        }
        let task = tokio::spawn(async move {
            worker
                .run(async {
                    let _ = stopped.await;
                })
                .await
        });
        // The worker subscribes before it serves, so wait until its subjects have listeners.
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(Self {
            env,
            nats,
            client,
            worker_id,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// Asks the worker to stop and waits for it to finish.
    pub async fn stop(&mut self) -> TestResult {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }

    /// Sends one JSON request and returns the JSON reply.
    pub async fn ask(&self, subject: String, payload: &Value) -> Boxed<Value> {
        let reply = tokio::time::timeout(
            Duration::from_secs(20),
            self.client.request(subject, serde_json::to_vec(payload)?.into()),
        )
        .await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    /// Dispatches a lease to the worker's queue group.
    pub async fn dispatch(&self, lease: &str, request: Value) -> Boxed<Value> {
        self.ask(
            "sandbox.provider.modal.lease".to_owned(),
            &json!({"lease_id": lease, "request": request}),
        )
        .await
    }
}

/// A local git repository with one commit, and that commit's id.
pub fn git_repo() -> Boxed<(tempfile::TempDir, String)> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("README.md"), "hello")?;
    let git = |args: &[&str]| -> Boxed<String> {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir.path())
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    git(&["init", "-q"])?;
    let blob = git(&["hash-object", "-w", "README.md"])?;
    git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},README.md"),
    ])?;
    let tree = git(&["write-tree"])?;
    let body = format!(
        "tree {tree}\nauthor test <test@example.com> 0 +0000\ncommitter test <test@example.com> 0 +0000\n\ninit\n"
    );
    std::fs::write(dir.path().join("commit.txt"), body)?;
    let sha = git(&["hash-object", "-t", "commit", "-w", "commit.txt"])?;
    std::fs::remove_file(dir.path().join("commit.txt"))?;
    git(&["update-ref", "HEAD", &sha])?;
    Ok((dir, sha))
}

/// The remote paths written into sandboxes, sorted.
pub fn written_paths(api: &FakeModal) -> Vec<String> {
    let mut paths: Vec<String> = api.with(|state| state.writes.iter().map(|(_, path, _)| path.clone()).collect());
    paths.sort();
    paths
}
