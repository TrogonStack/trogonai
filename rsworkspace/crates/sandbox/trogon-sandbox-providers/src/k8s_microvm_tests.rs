use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, Container, ContainerState, ContainerStateWaiting, ContainerStatus, Pod, PodSpec, Secret,
    SecretVolumeSource, SecurityContext, Volume,
};
use k8s_openapi::api::networking::v1::{NetworkPolicy as K8sNetworkPolicy, NetworkPolicySpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference, Time};
use k8s_openapi::jiff::Timestamp;
use tokio::sync::watch;
use trogon_sandbox_contract::{
    BootMode, CloneAuth, CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope,
    CredentialTransform, DomainPattern, DomainRule, DomainRuleAction, HeaderName, IsolationTier, NetworkPolicy,
    OutputRetention, PrivilegedAccess, PullSecretId, SecretRef, UnixSignal,
};
use trogon_sandbox_protocol::{CA_TRUST_ENV_VARS, DEFAULT_MAX_OUTPUT_BYTES};

use crate::k8s_microvm_handle::K8sMicroVmHandle;
use crate::k8s_ops::clone_secret_name;
use crate::k8s_ops::constants::{GIT_INIT_IMAGE, WORKSPACE_PATH};
use crate::*;
use std::sync::atomic::AtomicBool;

type FixtureResult<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = FixtureResult<()>;
type OpsResult<T> = Result<T, K8sOpsError>;

trait LockExt<T> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The mock cluster's fixed apiserver clock. Fixed, not `now()`: the point
/// of these paths is that they never read the host's clock.
fn mock_clock() -> ApiServerClock {
    Timestamp::from_second(1_704_067_200).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// A subscriber that enables every event, so the fields a `tracing` macro
/// reads are evaluated the way they are in a process that logs.
struct AlwaysOn;

impl tracing::Subscriber for AlwaysOn {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, _event: &tracing::Event<'_>) {}

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

struct NoopSink;

#[async_trait]
impl ArtifactSink for NoopSink {
    async fn put(
        &self,
        tool: &str,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactRef, ArtifactSinkError> {
        Ok(ArtifactRef {
            tool: tool.to_string(),
            object_bucket: "noop".to_string(),
            object_key: key.to_string(),
            sha256: String::new(),
            size: bytes.len() as u64,
            content_type: content_type.to_string(),
        })
    }

    async fn get(&self, artifact: &ArtifactRef) -> Result<Vec<u8>, ArtifactSinkError> {
        Err(ArtifactSinkError::GetUnsupported {
            object_key: artifact.object_key.clone(),
        })
    }
}

fn sample_spec(egress: EgressPolicy) -> FixtureResult<SandboxSpec> {
    Ok(SandboxSpec {
        image: ImageRef::new("sandbox-agent:latest"),
        cpu: 2.0,
        memory_mib: 1024,
        timeout: Duration::from_secs(300),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "acme".to_string(),
            name: "widget".to_string(),
        },
        resolved_sha: CommitSha::new("a".repeat(40))?,
        base_sha: None,
        tools: vec![
            ToolSpec {
                selector: ToolSelector("diff".to_string()),
                args: vec![],
            },
            ToolSpec {
                selector: ToolSelector("entities".to_string()),
                args: vec![],
            },
        ],
        artifact_sink: Arc::new(NoopSink),
        egress_policy: egress,
        network_policy: None,
        clone: None,
        host_mount: None,
        extra_binds: vec![],
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::Denied,
        exposed_ports: vec![],
        env: BTreeMap::default(),
        lease_id: None,
        disk_mib: None,
    })
}

fn sample_spec_with_base(egress: EgressPolicy) -> FixtureResult<SandboxSpec> {
    let mut spec = sample_spec(egress)?;
    spec.base_sha = Some(CommitSha::new("b".repeat(40))?);
    Ok(spec)
}

fn sample_spec_with_clone(egress: EgressPolicy) -> FixtureResult<SandboxSpec> {
    let mut spec = sample_spec(egress)?;
    spec.clone = Some(CloneCredentials {
        clone_url: "https://github.com/acme/widget.git".to_string(),
        auth: CloneAuth::BearerToken {
            token_ref: SecretRef::Literal {
                value: "test-token".to_string(),
            },
        },
    });
    Ok(spec)
}

#[derive(Debug)]
enum MockResponse {
    Ok,
    NotFound,
}

/// A recorded ConfigMap patch: namespace, name and the new data.
type ConfigMapPatch = (String, String, BTreeMap<String, String>);

#[derive(Default)]
struct RemoteExecLifetime {
    started: tokio::sync::Notify,
    release: tokio_util::sync::CancellationToken,
    running: std::sync::atomic::AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    signal_attempts: std::sync::atomic::AtomicUsize,
}

struct RemoteExecRunning(Arc<RemoteExecLifetime>);

impl Drop for RemoteExecRunning {
    fn drop(&mut self) {
        self.0.running.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

struct MockK8sOps {
    created_jobs: Mutex<Vec<Job>>,
    created_pods: Mutex<Vec<Pod>>,
    created_secrets: Mutex<Vec<Secret>>,
    created_network_policies: Mutex<Vec<K8sNetworkPolicy>>,
    created_config_maps: Mutex<Vec<ConfigMap>>,
    patched_config_maps: Mutex<Vec<ConfigMapPatch>>,
    patched_network_policies: Mutex<Vec<(String, String, NetworkPolicySpec)>>,
    secret_owners: Mutex<Vec<(String, OwnerReference)>>,
    delete_calls: Mutex<Vec<(String, String)>>,
    responses: Mutex<Vec<MockResponse>>,
    exec_calls: Mutex<Vec<Vec<String>>>,
    /// When set, an exec whose script is not a signal request never
    /// completes, standing in for a command that outlives its deadline.
    exec_hangs: std::sync::atomic::AtomicBool,
    signal_errors: std::sync::atomic::AtomicBool,
    signal_hangs: std::sync::atomic::AtomicBool,
    signal_keeps_exec_running: std::sync::atomic::AtomicBool,
    remote_exec: Mutex<Option<Arc<RemoteExecLifetime>>>,
    exec_output: Mutex<Option<PodExecOutput>>,
    /// The apiserver clock `list_jobs` and `list_pods` report.
    list_clock: Mutex<Option<ApiServerClock>>,
    /// When set, every created object carries the namespace and uid the
    /// apiserver stamps, which the UID-fenced receipts require.
    stamp_identity: AtomicBool,
    /// When set, created jobs and pods carry no uid.
    uidless: AtomicBool,
    /// Names whose deletion the apiserver refuses.
    failing_deletes: Mutex<BTreeSet<String>>,
    /// Every `(resource, owner)` pair `set_resource_owner` accepted.
    resource_owners: Mutex<Vec<(K8sResourceReceipt, K8sResourceReceipt)>>,
    /// When set, `set_secret_owner` is refused.
    owner_errors: AtomicBool,
    /// When set, a fenced claim loses the race to another worker.
    claim_conflicts: AtomicBool,
    /// When set, a fenced claim fails outright.
    claim_errors: AtomicBool,
}

impl MockK8sOps {
    fn new(responses: Vec<MockResponse>) -> Arc<Self> {
        // Only the first call installs it; a later one finds it set.
        let _ = tracing::subscriber::set_global_default(AlwaysOn);
        Arc::new(Self {
            created_jobs: Mutex::new(vec![]),
            created_pods: Mutex::new(vec![]),
            created_secrets: Mutex::new(vec![]),
            created_network_policies: Mutex::new(vec![]),
            created_config_maps: Mutex::new(vec![]),
            patched_config_maps: Mutex::new(vec![]),
            patched_network_policies: Mutex::new(vec![]),
            secret_owners: Mutex::new(vec![]),
            delete_calls: Mutex::new(vec![]),
            responses: Mutex::new(responses),
            exec_calls: Mutex::new(vec![]),
            exec_hangs: std::sync::atomic::AtomicBool::new(false),
            signal_errors: std::sync::atomic::AtomicBool::new(false),
            signal_hangs: std::sync::atomic::AtomicBool::new(false),
            signal_keeps_exec_running: std::sync::atomic::AtomicBool::new(false),
            remote_exec: Mutex::new(None),
            exec_output: Mutex::new(None),
            list_clock: Mutex::new(Some(mock_clock())),
            stamp_identity: AtomicBool::new(false),
            uidless: AtomicBool::new(false),
            failing_deletes: Mutex::new(BTreeSet::new()),
            resource_owners: Mutex::new(vec![]),
            owner_errors: AtomicBool::new(false),
            claim_conflicts: AtomicBool::new(false),
            claim_errors: AtomicBool::new(false),
        })
    }

    /// Stamps what the apiserver adds to every accepted object.
    fn stamp(&self, namespace: &str, metadata: &mut ObjectMeta) {
        if self.stamp_identity.load(Ordering::SeqCst) {
            metadata.namespace = Some(namespace.to_string());
            metadata.uid = Some(format!("uid-{}", metadata.name.as_deref().unwrap_or_default()));
        }
    }

    fn refuse_delete(&self, name: &str) -> OpsResult<()> {
        if self.failing_deletes.locked().contains(name) {
            return Err(K8sOpsError::Other(format!("fixture refused to delete {name}")));
        }
        Ok(())
    }

    fn resource_owners(&self) -> Vec<(K8sResourceReceipt, K8sResourceReceipt)> {
        self.resource_owners.locked().clone()
    }

    fn created_jobs(&self) -> Vec<Job> {
        self.created_jobs.locked().clone()
    }

    fn created_pods(&self) -> Vec<Pod> {
        self.created_pods.locked().clone()
    }

    /// Seeds a pod verbatim, bypassing the stamping `create_pod` does, so a
    /// test can present an object the apiserver would never have produced.
    fn seed_pod(&self, pod: Pod) {
        self.created_pods.locked().push(pod);
    }

    /// Moves every pod to `Running`, which is what `claim_warm_pod`
    /// requires and what a real pod reaches only after scheduling.
    fn mark_pods_running(&self) {
        for pod in self.created_pods.locked().iter_mut() {
            pod.status = Some(k8s_openapi::api::core::v1::PodStatus {
                phase: Some("Running".to_string()),
                ..Default::default()
            });
        }
    }

    fn pod_named(&self, name: &str) -> FixtureResult<Pod> {
        Ok(self
            .created_pods
            .locked()
            .iter()
            .find(|p| p.metadata.name.as_deref() == Some(name))
            .ok_or("no such pod")?
            .clone())
    }

    fn created_secrets(&self) -> Vec<Secret> {
        self.created_secrets.locked().clone()
    }

    fn created_network_policies(&self) -> Vec<K8sNetworkPolicy> {
        self.created_network_policies.locked().clone()
    }

    fn created_config_maps(&self) -> Vec<ConfigMap> {
        self.created_config_maps.locked().clone()
    }

    fn patched_config_maps(&self) -> Vec<ConfigMapPatch> {
        self.patched_config_maps.locked().clone()
    }

    fn patched_network_policies(&self) -> Vec<(String, String, NetworkPolicySpec)> {
        self.patched_network_policies.locked().clone()
    }

    fn secret_owners(&self) -> Vec<(String, OwnerReference)> {
        self.secret_owners.locked().clone()
    }

    fn delete_calls(&self) -> Vec<(String, String)> {
        self.delete_calls.locked().clone()
    }

    fn pop_response(&self) -> MockResponse {
        let mut r = self.responses.locked();
        if r.is_empty() {
            return MockResponse::Ok;
        }
        r.remove(0)
    }
}

#[async_trait]
impl K8sOps for MockK8sOps {
    async fn create_job(&self, namespace: &str, mut job: Job) -> OpsResult<Job> {
        let _ = self.pop_response();
        job.metadata.creation_timestamp = Some(Time(mock_clock()));
        job.metadata.uid = Some(format!("uid-{}", job.metadata.name.as_deref().unwrap_or_default()));
        self.stamp(namespace, &mut job.metadata);
        if self.uidless.load(Ordering::SeqCst) {
            job.metadata.uid = None;
        }
        self.created_jobs.locked().push(job.clone());
        Ok(job)
    }

    async fn create_pod(&self, namespace: &str, mut pod: Pod) -> OpsResult<Pod> {
        let _ = self.pop_response();
        if pod.metadata.name.is_none() {
            pod.metadata.name = Some(format!("wp-mock-{}", self.created_pods.locked().len()));
        }
        // The apiserver stamps these on every object it accepts, and the
        // fenced-claim path depends on both, so the mock stamps them too.
        pod.metadata.resource_version = Some("1".to_string());
        pod.metadata.creation_timestamp = Some(Time(mock_clock()));
        pod.metadata.uid = Some(format!("uid-{}", pod.metadata.name.as_deref().unwrap_or_default()));
        self.stamp(namespace, &mut pod.metadata);
        if self.uidless.load(Ordering::SeqCst) {
            pod.metadata.uid = None;
        }
        self.created_pods.locked().push(pod.clone());
        Ok(pod)
    }

    async fn create_secret(&self, namespace: &str, mut secret: Secret) -> OpsResult<Secret> {
        let _ = self.pop_response();
        self.stamp(namespace, &mut secret.metadata);
        self.created_secrets.locked().push(secret.clone());
        Ok(secret)
    }

    async fn put_secret(&self, _namespace: &str, secret: Secret) -> OpsResult<()> {
        let mut secrets = self.created_secrets.locked();
        secrets.retain(|existing| existing.metadata.name != secret.metadata.name);
        secrets.push(secret);
        Ok(())
    }

    async fn read_secret_key(&self, _namespace: &str, name: &str, key: &str) -> OpsResult<Option<Vec<u8>>> {
        Ok(self
            .created_secrets
            .locked()
            .iter()
            .find(|secret| secret.metadata.name.as_deref() == Some(name))
            .and_then(|secret| secret.data.as_ref()?.get(key).map(|bytes| bytes.0.clone())))
    }

    async fn create_network_policy(
        &self,
        namespace: &str,
        mut policy: K8sNetworkPolicy,
    ) -> OpsResult<K8sNetworkPolicy> {
        let _ = self.pop_response();
        self.stamp(namespace, &mut policy.metadata);
        self.created_network_policies.locked().push(policy.clone());
        Ok(policy)
    }

    async fn create_config_map(&self, namespace: &str, mut config_map: ConfigMap) -> OpsResult<ConfigMap> {
        let _ = self.pop_response();
        self.stamp(namespace, &mut config_map.metadata);
        self.created_config_maps.locked().push(config_map.clone());
        Ok(config_map)
    }

    async fn delete_job(&self, namespace: &str, name: &str) -> OpsResult<()> {
        self.delete_calls
            .locked()
            .push((namespace.to_string(), name.to_string()));
        self.refuse_delete(name)
    }

    async fn delete_pod(&self, namespace: &str, name: &str) -> OpsResult<()> {
        self.delete_calls
            .locked()
            .push((namespace.to_string(), name.to_string()));
        self.refuse_delete(name)
    }

    async fn delete_secret(&self, namespace: &str, name: &str) -> OpsResult<()> {
        self.delete_calls
            .locked()
            .push((namespace.to_string(), name.to_string()));
        self.refuse_delete(name)
    }

    async fn delete_network_policy(&self, namespace: &str, name: &str) -> OpsResult<()> {
        self.delete_calls
            .locked()
            .push((namespace.to_string(), name.to_string()));
        self.refuse_delete(name)
    }

    async fn find_pod_for_job(&self, _namespace: &str, job_name: &str) -> OpsResult<String> {
        Ok(format!("{job_name}-pod"))
    }

    async fn list_pods(&self, _namespace: &str, _label_selector: &str) -> OpsResult<PodListing> {
        Ok(PodListing {
            pods: self.created_pods.locked().clone(),
            clock: *self.list_clock.locked(),
        })
    }

    async fn list_jobs(&self, _namespace: &str, _label_selector: &str) -> OpsResult<JobListing> {
        Ok(JobListing {
            jobs: self.created_jobs.locked().clone(),
            clock: *self.list_clock.locked(),
        })
    }

    /// Models the apiserver's optimistic concurrency: a patch carrying a
    /// stale `resourceVersion` is refused, and an accepted one bumps it.
    async fn patch_pod_labels(
        &self,
        _namespace: &str,
        name: &str,
        labels: BTreeMap<String, String>,
        resource_version: &str,
    ) -> OpsResult<FencedPatchOutcome> {
        if self.claim_errors.load(Ordering::SeqCst) {
            return Err(K8sOpsError::Other("fixture refused the claim".to_string()));
        }
        if self.claim_conflicts.load(Ordering::SeqCst) {
            return Ok(FencedPatchOutcome::PreconditionFailed);
        }
        let mut pods = self.created_pods.locked();
        let Some(pod) = pods.iter_mut().find(|p| p.metadata.name.as_deref() == Some(name)) else {
            return Ok(FencedPatchOutcome::Applied);
        };
        if pod.metadata.resource_version.as_deref() != Some(resource_version) {
            return Ok(FencedPatchOutcome::PreconditionFailed);
        }
        pod.metadata.labels = Some(labels);
        pod.metadata.resource_version = Some(
            resource_version
                .parse::<u64>()
                .map(|v| (v + 1).to_string())
                .unwrap_or_else(|_| format!("{resource_version}x")),
        );
        Ok(FencedPatchOutcome::Applied)
    }

    async fn patch_config_map(&self, namespace: &str, name: &str, data: BTreeMap<String, String>) -> OpsResult<()> {
        self.patched_config_maps
            .locked()
            .push((namespace.to_string(), name.to_string(), data));
        Ok(())
    }

    async fn set_secret_owner(&self, _namespace: &str, name: &str, owner: OwnerReference) -> OpsResult<()> {
        if self.owner_errors.load(Ordering::SeqCst) {
            return Err(K8sOpsError::Other("fixture refused the owner".to_string()));
        }
        self.secret_owners.locked().push((name.to_string(), owner));
        Ok(())
    }

    async fn set_resource_owner(&self, receipt: &K8sResourceReceipt, owner: &K8sResourceReceipt) -> OpsResult<()> {
        self.resource_owners.locked().push((receipt.clone(), owner.clone()));
        Ok(())
    }

    async fn patch_network_policy(&self, namespace: &str, name: &str, spec: NetworkPolicySpec) -> OpsResult<()> {
        self.patched_network_policies
            .locked()
            .push((namespace.to_string(), name.to_string(), spec));
        Ok(())
    }

    async fn exec_collect_stdout(
        &self,
        _namespace: &str,
        _pod_name: &str,
        _container: &str,
        _command: Vec<String>,
    ) -> OpsResult<Vec<u8>> {
        Ok(vec![])
    }

    async fn exec_with_output(
        &self,
        _namespace: &str,
        _pod_name: &str,
        _container: &str,
        command: Vec<String>,
        _stdin_data: Option<Vec<u8>>,
        _max_output_bytes: usize,
    ) -> OpsResult<PodExecOutput> {
        let is_signal = command.iter().any(|arg| arg.contains("/proc/[0-9]*/environ"));
        self.exec_calls.locked().push(command);
        let remote = self.remote_exec.locked().clone();
        if is_signal {
            if let Some(remote) = &remote {
                remote.signal_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            if self.signal_errors.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(K8sOpsError::Other("fixture marker signal API unavailable".to_string()));
            }
            if self.signal_hangs.load(std::sync::atomic::Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if !self.signal_keeps_exec_running.load(std::sync::atomic::Ordering::SeqCst)
                && let Some(remote) = &remote
            {
                remote.release.cancel();
            }
        }
        if !is_signal && self.exec_hangs.load(std::sync::atomic::Ordering::SeqCst) {
            if let Some(remote) = remote {
                let task = tokio::spawn({
                    let remote = remote.clone();
                    async move {
                        let _running = RemoteExecRunning(remote.clone());
                        remote.running.store(true, std::sync::atomic::Ordering::SeqCst);
                        remote.started.notify_one();
                        remote.release.cancelled().await;
                    }
                });
                *remote.task.locked() = Some(task);
                remote.started.notified().await;
                remote.release.cancelled().await;
                return Ok(PodExecOutput {
                    exit_code: 137,
                    stdout: b"native-end-sentinel".to_vec(),
                    ..Default::default()
                });
            }
            std::future::pending::<()>().await;
        }
        Ok(self
            .exec_output
            .locked()
            .clone()
            .filter(|_| !is_signal)
            .unwrap_or_default())
    }

    async fn exec_interactive(
        &self,
        _namespace: &str,
        _pod_name: &str,
        _container: &str,
        _command: Vec<String>,
        _retention: OutputRetention,
        _max_output_bytes: usize,
    ) -> OpsResult<InteractivePodExec> {
        Ok(InteractivePodExec {
            stdin: Box::pin(tokio::io::sink()),
            completion: Box::pin(async { Ok(PodExecOutput::default()) }),
        })
    }

    async fn exec_tty(
        &self,
        _namespace: &str,
        _pod_name: &str,
        _container: &str,
        _command: Vec<String>,
        _size: (u16, u16),
    ) -> OpsResult<TtyPodExec> {
        let (resize, _rx) = tokio::sync::mpsc::channel(1);
        Ok(TtyPodExec {
            stdin: Box::pin(tokio::io::sink()),
            stdout: Box::pin(tokio::io::empty()),
            resize,
            completion: Box::pin(async { Ok(0) }),
        })
    }

    fn spawn_job_watcher(
        &self,
        _namespace: &str,
        _name: &str,
        _tx: watch::Sender<SandboxStatus>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async {})
    }
}

fn is_valid_label_value(value: &str) -> bool {
    value.len() <= 63
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && value.chars().next().is_none_or(|c| c.is_ascii_alphanumeric())
        && value.chars().last().is_none_or(|c| c.is_ascii_alphanumeric())
}

fn find_env(container: &Container, name: &str) -> Option<String> {
    container
        .env
        .as_ref()?
        .iter()
        .find(|e| e.name == name)
        .and_then(|e| e.value.clone())
}

fn provider(mock: Arc<MockK8sOps>) -> K8sMicroVmProvider {
    K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_runtime_class("kata-fc"),
    )
}

fn agent_container(job: &Job) -> FixtureResult<&Container> {
    Ok(pod_spec(job)?.containers.first().ok_or("no containers")?)
}

fn pod_spec(job: &Job) -> FixtureResult<&PodSpec> {
    Ok(job
        .spec
        .as_ref()
        .ok_or("job has no spec")?
        .template
        .spec
        .as_ref()
        .ok_or("job has no pod spec")?)
}

fn job_spec(job: &Job) -> FixtureResult<&JobSpec> {
    Ok(job.spec.as_ref().ok_or("job has no spec")?)
}

async fn deadline_requires_confirmed_signal_or_an_explicit_error(signal_hangs: bool) -> TestResult {
    let mock = MockK8sOps::new(vec![]);
    let remote = Arc::new(RemoteExecLifetime::default());
    *mock.remote_exec.locked() = Some(remote.clone());
    mock.exec_hangs.store(true, Ordering::SeqCst);
    mock.signal_errors.store(!signal_hangs, Ordering::SeqCst);
    mock.signal_hangs.store(signal_hangs, Ordering::SeqCst);
    let result = provider(mock.clone())
        .exec_in_pod_bounded(
            "pod",
            vec!["fixture-held-remote-exec".into()],
            "SANDBOX_EXECUTION_ID=timeout-ownership",
            Duration::from_secs(3),
            1024,
        )
        .await;
    let still_running = remote.running.load(Ordering::SeqCst);
    let signal_attempts = remote.signal_attempts.load(Ordering::SeqCst);
    remote.release.cancel();
    let task = remote.task.locked().take().ok_or("missing value")?;
    task.await?;
    assert!(
        still_running,
        "the failed signaling fixture must positively retain its independent remote execution"
    );
    assert_eq!(
        signal_attempts, 1,
        "the public bounded execution must reach its marker kill attempt"
    );
    assert!(
        result.is_err(),
        "an unconfirmed kill cannot become a terminal timed_out output while the remote execution remains alive: {result:?}"
    );
    Ok(())
}

fn job_pod(name: &str, phase: &str, agent: ContainerState) -> Pod {
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        status: Some(k8s_openapi::api::core::v1::PodStatus {
            phase: Some(phase.to_string()),
            container_statuses: Some(vec![ContainerStatus {
                name: "agent".to_string(),
                state: Some(agent),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn running() -> ContainerState {
    ContainerState {
        running: Some(Default::default()),
        ..Default::default()
    }
}

fn waiting(reason: &str) -> ContainerState {
    ContainerState {
        waiting: Some(ContainerStateWaiting {
            reason: Some(reason.to_string()),
            message: Some("detail".to_string()),
        }),
        ..Default::default()
    }
}

/// A lease Job as the apiserver returns it: created `age_secs` before the
/// mock clock, with `deadline_secs` as its `activeDeadlineSeconds`.
fn lease_job(name: &str, lease_id: &str, age_secs: i64, deadline_secs: i64) -> FixtureResult<Job> {
    let created = mock_clock().checked_sub(k8s_openapi::jiff::SignedDuration::from_secs(age_secs))?;
    Ok(Job {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(BTreeMap::from([
                (LABEL_LEASE_ID.to_string(), lease_id.to_string()),
                (LABEL_TENANT_ID.to_string(), "tenant-a".to_string()),
            ])),
            creation_timestamp: Some(Time(created)),
            ..Default::default()
        },
        spec: Some(JobSpec {
            active_deadline_seconds: Some(deadline_secs),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn finished(mut job: Job, condition: &str) -> Job {
    job.status = Some(k8s_openapi::api::batch::v1::JobStatus {
        conditions: Some(vec![k8s_openapi::api::batch::v1::JobCondition {
            type_: condition.to_string(),
            status: "True".to_string(),
            ..Default::default()
        }]),
        ..Default::default()
    });
    job
}

fn seed_job(mock: &MockK8sOps, job: Job) {
    mock.created_jobs.locked().push(job);
}

fn deleted_names(mock: &MockK8sOps) -> Vec<String> {
    mock.delete_calls().into_iter().map(|(_, name)| name).collect()
}

fn replica(mock: Arc<MockK8sOps>, identity: &str, recover_unlabeled: bool) -> K8sMicroVmProvider {
    K8sMicroVmProvider::new(
        Arc::clone(&mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default")
            .with_runtime_class("kata-fc")
            .with_worker_identity(identity)
            .with_recover_unlabeled_leases(recover_unlabeled),
    )
}

fn owned_by(mut job: Job, identity: &str) -> Job {
    job.metadata
        .labels
        .get_or_insert_with(BTreeMap::new)
        .insert(LABEL_WORKER_IDENTITY.to_string(), identity.to_string());
    job
}

fn live_lease_ids(sweep: &LeaseJobSweep) -> Vec<String> {
    let mut ids: Vec<String> = sweep.live.iter().map(|j| j.lease_id.clone()).collect();
    ids.sort();
    ids
}

/// A warm pod claimed for `lease_id` by `identity`, with a restore Secret
/// and, when given, a lease deadline `deadline_offset` seconds from the
/// mock clock.
fn claimed_warm_pod_due(name: &str, lease_id: &str, identity: &str, deadline_offset: Option<i64>) -> Pod {
    let mut labels = BTreeMap::from([
        (
            "app.kubernetes.io/managed-by".to_string(),
            K8sResourceNaming::Sandbox.managed_by().to_string(),
        ),
        (LABEL_WARM_POOL.to_string(), WARM_POOL_CLAIMED.to_string()),
        (LABEL_LEASE_ID.to_string(), lease_id.to_string()),
        (LABEL_WORKER_IDENTITY.to_string(), identity.to_string()),
    ]);
    if let Some(offset) = deadline_offset {
        labels.insert(
            LABEL_LEASE_DEADLINE.to_string(),
            (mock_clock().as_second() + offset).to_string(),
        );
    }
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(PodSpec {
            volumes: Some(vec![Volume {
                name: RESTORE_VOLUME_NAME.to_string(),
                secret: Some(SecretVolumeSource {
                    secret_name: Some(format!("{name}-restore")),
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn worker_pod(name: &str) -> Pod {
    Pod {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn aged(mut pod: Pod, age_secs: i64) -> FixtureResult<Pod> {
    let created = mock_clock().checked_sub(k8s_openapi::jiff::SignedDuration::from_secs(age_secs))?;
    pod.metadata.creation_timestamp = Some(Time(created));
    Ok(pod)
}

fn without_deadline(mut job: Job) -> FixtureResult<Job> {
    job.spec.as_mut().ok_or("job has no spec")?.active_deadline_seconds = None;
    Ok(job)
}

fn without_owner(mut pod: Pod) -> FixtureResult<Pod> {
    pod.metadata
        .labels
        .as_mut()
        .ok_or("pod has no labels")?
        .remove(LABEL_WORKER_IDENTITY);
    Ok(pod)
}

/// Moves the apiserver clock the fake reports to `secs` after the start.
fn clock_at(mock: &MockK8sOps, secs: i64) -> FixtureResult<()> {
    *mock.list_clock.locked() = Some(mock_clock().checked_add(k8s_openapi::jiff::SignedDuration::from_secs(secs))?);
    Ok(())
}

/// Sweep instants up to `end`, spaced the way the sweeper's own interval
/// spaces them, starting with the first sighting at zero.
fn sweep_times(end: i64) -> Vec<i64> {
    (0..=end).step_by(300).collect()
}

fn remove_pod(mock: &MockK8sOps, name: &str) {
    mock.created_pods
        .locked()
        .retain(|pod| pod.metadata.name.as_deref() != Some(name));
}

struct FixedImageUser(Result<Option<String>, String>);

#[async_trait]
impl ImageUserResolver for FixedImageUser {
    async fn image_user(&self, _image: &str) -> OpsResult<Option<String>> {
        self.0.clone().map_err(K8sOpsError::Other)
    }
}

fn honoring_provider(mock: &Arc<MockK8sOps>, image_user: Result<Option<String>, String>) -> K8sMicroVmProvider {
    K8sMicroVmProvider::new(
        Arc::clone(mock) as Arc<dyn K8sOps>,
        K8sMicroVmConfig::new("default").with_honor_image_user(true),
    )
    .with_image_user_resolver(Arc::new(FixedImageUser(image_user)))
}

async fn launched_agent_security_context(
    mock: &Arc<MockK8sOps>,
    provider: K8sMicroVmProvider,
) -> FixtureResult<SecurityContext> {
    provider.launch(sample_spec(EgressPolicy::DenyAll)?).await?;
    let jobs = mock.created_jobs();
    Ok(pod_spec(&jobs[0])?
        .containers
        .iter()
        .find(|c| c.name == "agent")
        .ok_or("no agent container")?
        .security_context
        .clone()
        .ok_or("no security context")?)
}

struct NeverAnswers;

#[async_trait]
impl ImageUserResolver for NeverAnswers {
    async fn image_user(&self, _image: &str) -> OpsResult<Option<String>> {
        std::future::pending().await
    }
}

struct CountingImageUser(std::sync::atomic::AtomicUsize, Result<Option<String>, String>);

#[async_trait]
impl ImageUserResolver for CountingImageUser {
    async fn image_user(&self, _image: &str) -> OpsResult<Option<String>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        self.1.clone().map_err(K8sOpsError::Other)
    }
}

fn assert_bounded_runtime_tmp(pod: &PodSpec, requested_memory_mib: u32) -> TestResult {
    let agent = pod
        .containers
        .iter()
        .find(|container| container.name == "agent")
        .ok_or("missing value")?;
    assert_eq!(
        agent
            .security_context
            .as_ref()
            .and_then(|security| security.read_only_root_filesystem),
        Some(true),
        "runtime scratch must preserve the read-only root filesystem"
    );
    let mounts = agent.volume_mounts.as_ref().ok_or("missing value")?;
    let scratch: Vec<_> = mounts.iter().filter(|mount| mount.mount_path == "/tmp").collect();
    assert_eq!(
        scratch.len(),
        1,
        "native terminal custody needs one writable per-pod /tmp mount"
    );
    assert_ne!(scratch[0].read_only, Some(true));
    assert_ne!(
        scratch[0].name, "workspace",
        "ephemeral terminal custody must stay outside workspace snapshots"
    );
    assert_ne!(scratch[0].name, "evidence");
    let volume = pod
        .volumes
        .as_ref()
        .ok_or("missing value")?
        .iter()
        .find(|volume| volume.name == scratch[0].name)
        .ok_or("missing value")?;
    assert!(volume.host_path.is_none());
    assert!(volume.persistent_volume_claim.is_none());
    let empty = volume.empty_dir.as_ref().ok_or("missing value")?;
    assert_eq!(
        empty.medium.as_deref(),
        Some("Memory"),
        "runtime scratch must participate in container memory accounting"
    );
    let quantity = empty.size_limit.as_ref().ok_or("missing value")?;
    let size_mib: u32 = quantity.0.strip_suffix("Mi").ok_or("missing value")?.parse()?;
    assert!(
        size_mib > 0,
        "an unspecified container memory request must still have bounded nonzero scratch"
    );
    if requested_memory_mib > 0 {
        assert!(
            size_mib <= requested_memory_mib,
            "scratch cannot exceed the existing requested memory budget"
        );
    }
    Ok(())
}

async fn claimed_warm_pod(mock: &Arc<MockK8sOps>, p: &K8sMicroVmProvider) -> FixtureResult<String> {
    let pod_name = p
        .create_warm_pod("sha256:abc", "img:1", "restore-1", "kata-fc", 0.25, 256)
        .await?;
    mock.mark_pods_running();
    p.claim_warm_pod("sha256:abc", "lease-a", "tenant-a", std::time::Duration::ZERO)
        .await?;
    Ok(pod_name)
}

fn pod_selector_labels_for(job_name: &str) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert(K8sResourceNaming::Sandbox.job_label().to_string(), job_name.to_string());
    labels
}

mod builders_tests;
mod exec_tests;
mod export_tests;
mod handle_tests;
mod image_user_tests;
mod job_wait_tests;
mod launch_inputs_tests;
mod launch_policy_tests;
mod launch_tests;
mod lease_request_tests;
mod naming_tests;
mod receipts_tests;
mod secrets_tests;
mod sweep_failure_tests;
mod sweep_tests;
mod warm_claim_tests;
mod warm_pool_tests;
