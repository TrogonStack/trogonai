use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_broker::active_lease::{ActiveLeaseStore, InMemoryActiveLeaseStore};
use trogon_sandbox_broker::command_launch::InMemoryCommandLaunchStore;
use trogon_sandbox_broker::core::{BrokerConfig, BrokerCore};
use trogon_sandbox_broker::default_capability_profile::default_capability_profile;
use trogon_sandbox_broker::digest_pinning::InMemorySnapshotPinStore;
use trogon_sandbox_broker::host_import_policy::InMemoryHostImportPolicy;
use trogon_sandbox_broker::host_mount_policy::InMemoryHostMountAllowList;
use trogon_sandbox_broker::idempotency::InMemoryIdempotencyStore;
use trogon_sandbox_broker::image_policy::InMemoryImageAllowList;
use trogon_sandbox_broker::limits::InMemoryTenantLimits;
use trogon_sandbox_broker::metrics::BrokerMetrics;
use trogon_sandbox_broker::named_lease::InMemoryNamedLeaseStore;
use trogon_sandbox_broker::nats_bus::NatsBus;
use trogon_sandbox_broker::quota::InMemoryQuotaStore;
use trogon_sandbox_broker::resolved_digest::InMemoryResolvedDigestStore;
use trogon_sandbox_broker::signing_bus::SigningBus;
use trogon_sandbox_broker::snapshot_tag::{InMemorySnapshotTagAuditor, InMemorySnapshotTagStore};
use trogon_sandbox_broker::worker_registry::{NatsWorkerRegistry, WorkerRegistry};
use trogon_sandbox_contract::{
    CapabilitySupport, CommandReadinessRequest, CommandReadinessResponse, CommandReadinessRuntime,
    CommandReadinessTarget, CommandReadinessUnsupportedReason, Digest, IsolationTier, LeaseGranted, LeaseRecord,
    LeaseRequest, LeaseResponse, OciImageRef, ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId,
    SandboxImage, SandboxSpec, SandboxSpecParts, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol,
    WorkerHeartbeat, WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH};

use crate::context::{Context, TestResult};

pub(crate) const WAIT: Duration = Duration::from_secs(5);
pub(crate) const PROVIDER: ProviderKind = ProviderKind::LocalDocker;
const READY_SHA: &str = "a234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
const SIGNING_SEED_BYTE: u8 = 47;

pub(crate) struct Harness {
    pub(crate) core: Arc<BrokerCore>,
    pub(crate) active: Arc<dyn ActiveLeaseStore>,
    registry: Arc<NatsWorkerRegistry>,
    pub(crate) verifier: DispatchVerifier,
    pub(crate) client: Client,
    cancel: CancellationToken,
    answer: Arc<Mutex<Option<Answer>>>,
    pub(crate) observed: Arc<Mutex<Option<CommandReadinessRequest>>>,
    _server: JetStreamTestServer,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn spec() -> TestResult<SandboxSpec> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("base"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: std::num::NonZeroU32::new(30).context("cpu seconds")?,
        memory_mib: std::num::NonZeroU32::new(512).context("memory mib")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
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
    })?)
}

impl Harness {
    pub(crate) async fn new() -> TestResult<Self> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let cancel = CancellationToken::new();
        let registry = NatsWorkerRegistry::new(client.clone(), Duration::from_secs(60), cancel.clone()).await?;
        client.flush().await?;
        let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
        let signer = Arc::new(DispatchSigner::new(
            "readiness-contract",
            [0u8; 32].map(|_| SIGNING_SEED_BYTE),
        ));
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_key(signer.key_id(), &signer.public_key_b64())?
            .with_epoch_mode(EpochMode::Require);
        let bus = Arc::new(SigningBus::new(
            Arc::new(NatsBus::new(client.clone())),
            active.clone(),
            signer.clone(),
        ));
        let core = BrokerCore::new(
            bus,
            Arc::new(InMemoryQuotaStore::new()),
            Arc::new(InMemoryIdempotencyStore::new()),
            active.clone(),
            Arc::new(InMemoryImageAllowList::new()),
            Arc::new(InMemoryHostMountAllowList::new()),
            Arc::new(InMemoryHostImportPolicy::new()),
            Arc::new(InMemoryTenantLimits::new()),
            registry.clone(),
            InMemorySnapshotPinStore::new(),
            Arc::new(InMemorySnapshotTagStore::new(Arc::new(
                InMemorySnapshotTagAuditor::new(),
            ))),
            Arc::new(InMemoryResolvedDigestStore::new()),
            Arc::new(trogon_sandbox_broker::core::InMemoryDriveStore::new()),
            Arc::new(InMemoryNamedLeaseStore::new()),
            BrokerConfig {
                default_cpu_cap: 3600,
                default_wall_cap: 7200,
                enforce_image_allowlist: false,
                enforce_worker_registry: true,
                default_max_cpu_per_request: 3600,
                default_max_memory_mib_per_request: 8192,
                default_active_lease_cap: 50,
                snapshot_registry: "registry.example.test".into(),
                provider_dispatch_timeout: WAIT,
            },
            BrokerMetrics::new()?,
        )
        .with_dispatch_signer(signer);
        let core = core.with_command_launch_store(Arc::new(InMemoryCommandLaunchStore::new()));
        Ok(Self {
            core: Arc::new(core),
            active,
            registry,
            verifier,
            client,
            cancel,
            answer: Arc::new(Mutex::new(None)),
            observed: Arc::new(Mutex::new(None)),
            _server: server,
        })
    }

    pub(crate) async fn answer(&self, answer: Answer) {
        *self.answer.lock().await = Some(answer);
    }

    pub(crate) async fn heartbeat(&self, worker: &WorkerId, strong: bool) -> TestResult<WorkerHeartbeat> {
        let mut profile = default_capability_profile();
        profile.provider = PROVIDER;
        profile.isolation.default_tier = IsolationTier::Container;
        profile.isolation.supported_tiers = vec![IsolationTier::Container];
        profile.isolation.supports_command_cancellation = CapabilitySupport::from(strong);
        profile.isolation.supports_command_launch_cancellation = CapabilitySupport::from(strong);
        let heartbeat = WorkerHeartbeat {
            worker_id: worker.clone(),
            provider: PROVIDER,
            capacity: WorkerCapacity {
                max_concurrent_leases: 4,
                current_leases: 0,
                capabilities: WorkerCapabilities::from(&profile),
            },
            profile,
            emitted_at: SystemTime::now(),
            cached_digests: Vec::new(),
            warm_pool_population: Default::default(),
            supports_lease_epoch: CapabilitySupport::Supported,
            dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
            native_volume_bindings: Vec::new(),
        };
        self.client
            .publish(
                subjects::worker_heartbeat(PROVIDER, worker),
                serde_json::to_vec(&heartbeat)?.into(),
            )
            .await?;
        self.client.flush().await?;
        tokio::time::timeout(WAIT, async {
            loop {
                if self
                    .registry
                    .current_worker(PROVIDER, worker)
                    .await
                    .is_some_and(|seen| seen.emitted_at == heartbeat.emitted_at)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .context("the production NATS registry must observe this exact heartbeat")?;
        Ok(heartbeat)
    }

    pub(crate) async fn lease(&self, worker: &WorkerId, key: &str) -> TestResult<(LeaseGranted, LeaseRecord)> {
        let mut dispatch = self
            .client
            .subscribe(subjects::worker_dispatch(PROVIDER, worker))
            .await?;
        let mut legacy = self.client.subscribe(subjects::provider_dispatch(PROVIDER)).await?;
        self.client.flush().await?;
        let answer = self.answer.lock().await.take();
        let mut readiness = if answer.is_some() {
            Some(
                self.client
                    .subscribe(subjects::worker_command_readiness_filter(PROVIDER, worker))
                    .await?,
            )
        } else {
            None
        };
        self.client.flush().await?;
        let request: LeaseRequest = serde_json::from_value(json!({
            "tenant":"readiness-tenant", "provider_pref":["local_docker"],
            "spec":spec()?,
            "ttl_seconds":60, "idempotency_key":key, "persistent":false,
        }))?;
        let task = {
            let core = self.core.clone();
            tokio::spawn(async move { core.handle_lease_request(request).await })
        };
        let message = tokio::time::timeout(WAIT, dispatch.next())
            .await
            .context("the broker must dispatch to the selected worker")?
            .context("the dispatch subscription ended")?;
        let payload: ProviderDispatchPayload =
            self.verifier
                .decode(&message.payload, OP_LEASE_DISPATCH, Some(worker))?;
        assert_eq!(message.subject.as_str(), subjects::worker_dispatch(PROVIDER, worker));
        let target = payload.target.clone().context("the dispatch carries its target")?;
        let receipt = ProviderLease {
            sandbox_id: SandboxId::new(format!("readiness-stub-{}", payload.lease_id))?,
            evidence_subject: subjects::evidence_subject(&payload.lease_id),
            worker_id: Some(target.worker_id),
            attempt_id: Some(target.attempt_id),
            epoch: target.epoch,
        };
        self.client
            .publish(
                message.reply.context("the dispatch carries a reply subject")?,
                serde_json::to_vec(&receipt)?.into(),
            )
            .await?;
        if let Some(answer) = answer {
            let message = tokio::time::timeout(
                WAIT,
                readiness.as_mut().context("the readiness subscription exists")?.next(),
            )
            .await
            .context("the broker must ask for readiness")?
            .context("the readiness subscription ended")?;
            let request: CommandReadinessRequest = serde_json::from_slice(&message.payload)?;
            *self.observed.lock().await = Some(request.clone());
            let body = answer.response(&request)?;
            self.client
                .publish(message.reply.context("readiness carries a reply subject")?, body.into())
                .await?;
        }
        let response = tokio::time::timeout(WAIT, task).await.context("the lease settles")??;
        let LeaseResponse::Granted(grant) = response else {
            return Err(format!("synthetic targeted receipt must grant: {response:?}").into());
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(100), legacy.next())
                .await
                .is_err()
        );
        let record = self
            .active
            .get(&grant.lease_id)
            .await?
            .context("the granted lease must be stored")?;
        Ok((grant, record))
    }

    pub(crate) fn readiness_binding(record: &LeaseRecord) -> TestResult<CommandReadinessRequest> {
        let attempt = record
            .dispatch_attempt
            .as_ref()
            .context("the lease record names its dispatch attempt")?;
        Ok(CommandReadinessRequest::new(
            record.tenant.clone(),
            record.lease_id.clone(),
            PROVIDER,
            CommandReadinessTarget {
                worker_id: record.worker_id.clone().context("the lease record names its worker")?,
                attempt_id: attempt.id.clone(),
                epoch: record.epoch.context("the lease record names its epoch")?,
            },
            SandboxId::new(record.sandbox_id.clone())?,
            None,
        )?)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Answer {
    Ready,
    Unsupported,
}

impl Answer {
    pub(crate) fn response(self, request: &CommandReadinessRequest) -> TestResult<Vec<u8>> {
        Ok(match self {
            Self::Ready => serde_json::to_vec(&CommandReadinessResponse::Ready {
                binding: request.clone(),
                runtime: CommandReadinessRuntime {
                    sha256: Digest::new(format!("sha256:{READY_SHA}"))?,
                    protocol: CommandReadinessRuntime::PROTOCOL,
                },
            })?,
            Self::Unsupported => serde_json::to_vec(&CommandReadinessResponse::Unsupported {
                binding: request.clone(),
                reason: CommandReadinessUnsupportedReason::IncompatibleIsolation,
            })?,
        })
    }
}
