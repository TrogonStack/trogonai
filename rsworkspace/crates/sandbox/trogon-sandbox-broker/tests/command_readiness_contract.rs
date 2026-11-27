//! Component-only broker contracts for per-lease command readiness.
//!
//! Workers in this fixture are synthetic NATS responders. They create no native
//! resource and cannot establish secure SDK or provider acceptance.

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
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CapabilitySupport, CommandReadinessDeniedReason,
    CommandReadinessRequest, CommandReadinessResponse, CommandReadinessRuntime, CommandReadinessTarget,
    CommandReadinessUnavailableReason, CommandReadinessUnsupportedReason, Digest, DispatchAttemptId, ExecutionId,
    FreezeAction, FreezeInFlight, IsolationTier, LaunchKey, LeaseEpoch, LeaseGranted, LeaseId, LeaseRecord,
    LeaseRequest, LeaseResponse, LeaseStatus, OciImageRef, ProviderDispatchPayload, ProviderKind, ProviderLease,
    RunCommandRequest, RunCommandResponse, SandboxId, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxUserName,
    SdkFeature, TenantId, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
    WorkspaceSource,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH,
};

#[path = "command_readiness_fresh_support/context.rs"]
mod context;

use context::{Context, TestResult};

const WAIT: Duration = Duration::from_secs(5);
const PROVIDER: ProviderKind = ProviderKind::LocalDocker;
const READY_SHA: &str = "a234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
const SIGNING_SEED_BYTE: u8 = 47;
const MAX_READY_BYTES: usize = 64 * 1024;

struct Harness {
    core: Arc<BrokerCore>,
    active: Arc<dyn ActiveLeaseStore>,
    registry: Arc<NatsWorkerRegistry>,
    verifier: DispatchVerifier,
    client: Client,
    cancel: CancellationToken,
    answer: Arc<Mutex<Option<Answer>>>,
    observed: Arc<Mutex<Option<CommandReadinessRequest>>>,
    query_mutation: Mutex<Option<QueryMutation>>,
    applied_mutation: Mutex<Option<QueryMutation>>,
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
    async fn new() -> TestResult<Self> {
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
            query_mutation: Mutex::new(None),
            applied_mutation: Mutex::new(None),
            _server: server,
        })
    }

    async fn answer(&self, answer: Answer) {
        *self.answer.lock().await = Some(answer);
    }

    async fn mutate_during_query(&self, mutation: QueryMutation) {
        *self.query_mutation.lock().await = Some(mutation);
    }

    async fn publish_heartbeat(&self, heartbeat: &WorkerHeartbeat) -> TestResult {
        self.client
            .publish(
                subjects::worker_heartbeat(PROVIDER, &heartbeat.worker_id),
                serde_json::to_vec(heartbeat)?.into(),
            )
            .await?;
        self.client.flush().await?;
        tokio::time::timeout(WAIT, async {
            loop {
                if self
                    .registry
                    .current_worker(PROVIDER, &heartbeat.worker_id)
                    .await
                    .is_some_and(|seen| {
                        seen.emitted_at == heartbeat.emitted_at
                            && seen.profile == heartbeat.profile
                            && seen.dispatch_protocol == heartbeat.dispatch_protocol
                            && seen.supports_lease_epoch == heartbeat.supports_lease_epoch
                    })
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .context("the production NATS registry must observe this exact heartbeat")?;
        Ok(())
    }

    async fn heartbeat_flags(&self, worker: &WorkerId, command: bool, launch: bool) -> TestResult<WorkerHeartbeat> {
        let mut heartbeat = self.heartbeat(worker, false).await?;
        heartbeat.profile.isolation.supports_command_cancellation = CapabilitySupport::from(command);
        heartbeat.profile.isolation.supports_command_launch_cancellation = CapabilitySupport::from(launch);
        heartbeat.capacity.capabilities = WorkerCapabilities::from(&heartbeat.profile);
        heartbeat.emitted_at = SystemTime::now();
        self.publish_heartbeat(&heartbeat).await?;
        Ok(heartbeat)
    }

    async fn change_heartbeat(
        &self,
        worker: &WorkerId,
        refresh_timestamp: bool,
        change: impl FnOnce(&mut WorkerHeartbeat),
    ) -> TestResult {
        let mut heartbeat = self
            .registry
            .current_worker(PROVIDER, worker)
            .await
            .context("the registry knows the worker")?;
        change(&mut heartbeat);
        if refresh_timestamp {
            heartbeat.emitted_at = SystemTime::now();
        }
        heartbeat.capacity.capabilities = WorkerCapabilities::from(&heartbeat.profile);
        self.publish_heartbeat(&heartbeat).await
    }

    async fn change_record(
        &self,
        lease_id: &LeaseId,
        change: impl FnOnce(&mut LeaseRecord) -> TestResult,
    ) -> TestResult {
        let mut record = self.active.get(lease_id).await?.context("the lease record exists")?;
        change(&mut record)?;
        self.active.put(&record).await?;
        Ok(())
    }

    async fn apply_query_mutation(&self, request: &CommandReadinessRequest) -> TestResult {
        let Some(mutation) = self.query_mutation.lock().await.take() else {
            return Ok(());
        };
        let worker = &request.target().worker_id;
        let lease = request.lease_id();
        match mutation {
            QueryMutation::ProfileWithdrawal => {
                self.change_heartbeat(worker, true, withdraw_strong_features).await?;
            }
            QueryMutation::SameTimestampProfileWithdrawal => {
                self.change_heartbeat(worker, false, withdraw_strong_features).await?;
            }
            QueryMutation::ForeignProfile => {
                self.change_heartbeat(worker, true, |heartbeat| {
                    heartbeat.profile.provider = ProviderKind::E2b;
                })
                .await?;
            }
            QueryMutation::LegacyHeartbeat => {
                self.change_heartbeat(worker, true, |heartbeat| {
                    heartbeat.dispatch_protocol = WorkerDispatchProtocol::Legacy;
                })
                .await?;
            }
            QueryMutation::EpochSupportWithdrawal => {
                self.change_heartbeat(worker, true, |heartbeat| {
                    heartbeat.supports_lease_epoch = CapabilitySupport::Unsupported;
                })
                .await?;
            }
            QueryMutation::HeartbeatRefresh => {
                self.change_heartbeat(worker, true, |_| {}).await?;
            }
            QueryMutation::Tenant => {
                self.change_record(lease, |record| {
                    record.tenant = TenantId::new("foreign-tenant")?;
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Provider => {
                self.change_record(lease, |record| {
                    record.provider = ProviderKind::E2b;
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Native => {
                self.change_record(lease, |record| {
                    record.sandbox_id = "foreign-native".into();
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Worker => {
                self.change_record(lease, |record| {
                    record.worker_id = Some(WorkerId::new("foreign-worker")?);
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Attempt => {
                self.change_record(lease, |record| {
                    record
                        .dispatch_attempt
                        .as_mut()
                        .context("the lease record names its dispatch attempt")?
                        .id = DispatchAttemptId::new("foreign-attempt")?;
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Epoch => {
                self.change_record(lease, |record| {
                    record.epoch = Some(LeaseEpoch::new(2)?);
                    record
                        .dispatch_attempt
                        .as_mut()
                        .context("the lease record names its dispatch attempt")?
                        .epoch = record.epoch;
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Expired => {
                self.change_record(lease, |record| {
                    record.deadline = SystemTime::now() - Duration::from_secs(1);
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Paused => {
                self.change_record(lease, |record| {
                    record.status = LeaseStatus::Paused;
                    Ok(())
                })
                .await?;
            }
            QueryMutation::Freeze => {
                self.change_record(lease, |record| {
                    record.freeze_in_flight = Some(FreezeInFlight {
                        action: FreezeAction::Pause,
                        started_at: SystemTime::now(),
                        settle_attempts: 0,
                        next_settle_at: None,
                    });
                    Ok(())
                })
                .await?;
            }
        }
        *self.applied_mutation.lock().await = Some(mutation);
        Ok(())
    }

    async fn heartbeat(&self, worker: &WorkerId, strong: bool) -> TestResult<WorkerHeartbeat> {
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

    async fn lease(&self, worker: &WorkerId, key: &str) -> TestResult<(LeaseGranted, LeaseRecord)> {
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
            let auth = message
                .headers
                .as_ref()
                .and_then(|headers| headers.get(AUTH_HEADER))
                .context("the readiness query carries its signature")?;
            self.verifier.verify_verb(
                message.subject.as_str(),
                auth.as_str(),
                &message.payload,
                false,
                SystemTime::now(),
            )?;
            *self.observed.lock().await = Some(request.clone());
            self.apply_query_mutation(&request).await?;
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

    fn readiness_binding(record: &LeaseRecord) -> TestResult<CommandReadinessRequest> {
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

fn withdraw_strong_features(heartbeat: &mut WorkerHeartbeat) {
    heartbeat.profile.isolation.supports_command_cancellation = CapabilitySupport::Unsupported;
    heartbeat.profile.isolation.supports_command_launch_cancellation = CapabilitySupport::Unsupported;
}

#[derive(Clone, Copy)]
enum Answer {
    Ready,
    Unsupported,
    Unavailable,
    Denied,
    Malformed,
    ForeignReady(BindingField),
    UnsupportedProtocol,
    OversizedReady,
}

#[derive(Clone, Copy, Debug)]
enum BindingField {
    Tenant,
    Provider,
    Lease,
    Native,
    Worker,
    Attempt,
    Epoch,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryMutation {
    Tenant,
    Provider,
    Native,
    Worker,
    Attempt,
    Epoch,
    Expired,
    Paused,
    Freeze,
    ProfileWithdrawal,
    SameTimestampProfileWithdrawal,
    ForeignProfile,
    LegacyHeartbeat,
    EpochSupportWithdrawal,
    HeartbeatRefresh,
}

fn foreign_binding(request: &CommandReadinessRequest, field: BindingField) -> TestResult<CommandReadinessRequest> {
    let mut tenant = request.tenant().clone();
    let mut provider = request.provider();
    let mut lease = request.lease_id().clone();
    let mut sandbox = request.sandbox_id().clone();
    let mut target = request.target().clone();
    let mut user = request.run_as_user().cloned();
    match field {
        BindingField::Tenant => tenant = TenantId::new("foreign-tenant")?,
        BindingField::Provider => provider = ProviderKind::E2b,
        BindingField::Lease => lease = LeaseId::new("foreign-lease"),
        BindingField::Native => sandbox = SandboxId::new("foreign-native")?,
        BindingField::Worker => target.worker_id = WorkerId::new("foreign-worker")?,
        BindingField::Attempt => target.attempt_id = DispatchAttemptId::new("foreign-attempt")?,
        BindingField::Epoch => target.epoch = LeaseEpoch::new(2)?,
        BindingField::User => user = Some(SandboxUserName::new("root")?),
    }
    Ok(CommandReadinessRequest::new(
        tenant, lease, provider, target, sandbox, user,
    )?)
}

impl Answer {
    fn response(self, request: &CommandReadinessRequest) -> TestResult<Vec<u8>> {
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
            Self::Unavailable => serde_json::to_vec(&CommandReadinessResponse::Unavailable {
                binding: request.clone(),
                reason: CommandReadinessUnavailableReason::ProbeFailed,
            })?,
            Self::Denied => serde_json::to_vec(&CommandReadinessResponse::Denied {
                reason: CommandReadinessDeniedReason::ForeignBinding,
            })?,
            Self::Malformed => b"{malformed".to_vec(),
            Self::ForeignReady(field) => Self::Ready.response(&foreign_binding(request, field)?)?,
            Self::UnsupportedProtocol => {
                let mut body: serde_json::Value = serde_json::from_slice(&Self::Ready.response(request)?)?;
                body["runtime"]["protocol"] = json!(2);
                serde_json::to_vec(&body)?
            }
            Self::OversizedReady => {
                let mut body = Self::Ready.response(request)?;
                body.resize(MAX_READY_BYTES + 1, b' ');
                body
            }
        })
    }
}

fn is_strong(feature: SdkFeature) -> bool {
    matches!(feature.as_str(), "command_cancellation" | "command_launch_cancellation")
}

fn all_features(features: &[SdkFeature]) -> Vec<String> {
    features.iter().map(|feature| feature.as_str().to_owned()).collect()
}

fn ordinary(features: &[SdkFeature]) -> Vec<String> {
    features
        .iter()
        .copied()
        .filter(|feature| !is_strong(*feature))
        .map(|feature| feature.as_str().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_masks_strong_features_when_per_lease_readiness_has_no_responder() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("readiness-no-reply")?;
    let heartbeat = harness.heartbeat(&worker, true).await?;
    let (grant, _) = harness.lease(&worker, "missing-readiness").await?;
    eprintln!(
        "component-only readiness no-responder receipt: worker={} strong_profile=true native_acceptance=false",
        worker
    );
    assert_eq!(
        grant.capabilities,
        ordinary(&heartbeat.profile.sdk_features()),
        "strong SDK features require an exact per-lease Ready reply"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_an_exact_ready_reply_may_project_strong_features_and_it_binds_the_request() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("readiness-ready")?;
    harness.heartbeat(&worker, true).await?;
    harness.answer(Answer::Ready).await;
    let (grant, record) = harness.lease(&worker, "ready-readiness").await?;
    assert_eq!(
        harness.observed.lock().await.as_ref(),
        Some(&Harness::readiness_binding(&record)?)
    );
    assert!(
        grant.capabilities.contains(&"command_cancellation".into())
            && grant.capabilities.contains(&"command_launch_cancellation".into()),
        "exact Ready must preserve both strong features"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_unavailable_denied_malformed_foreign_and_stale_readiness_all_mask_strong_features() -> TestResult {
    let outcomes = [
        ("unsupported", Answer::Unsupported),
        ("unavailable", Answer::Unavailable),
        ("denied", Answer::Denied),
        ("malformed", Answer::Malformed),
    ];
    for (label, response) in outcomes {
        let harness = Harness::new().await?;
        let worker = WorkerId::new(format!("readiness-{label}"))?;
        let heartbeat = harness.heartbeat(&worker, true).await?;
        harness.answer(response).await;
        let (grant, _) = harness.lease(&worker, &format!("{label}-readiness")).await?;
        assert_eq!(
            grant.capabilities,
            ordinary(&heartbeat.profile.sdk_features()),
            "{label} readiness must mask strong features"
        );
    }
    Ok(())
}

fn dummy_binding() -> TestResult<CommandReadinessRequest> {
    Ok(CommandReadinessRequest::new(
        TenantId::new("readiness-tenant")?,
        LeaseId::new("dummy"),
        PROVIDER,
        CommandReadinessTarget {
            worker_id: WorkerId::new("dummy-worker")?,
            attempt_id: DispatchAttemptId::new("dummy-attempt")?,
            epoch: LeaseEpoch::FIRST,
        },
        SandboxId::new("a".repeat(64))?,
        None,
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_cancellation_does_not_wait_for_a_new_readiness_query_after_withdrawal() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("readiness-retained")?;
    harness.heartbeat(&worker, true).await?;
    harness.answer(Answer::Ready).await;
    let (_grant, record) = harness.lease(&worker, "retained-readiness").await?;
    // The corrected fresh admission asks for one exact Ready before the first
    // strong launch. Consume that query here, then drop this responder before
    // the later withdrawal check so retained cancellation still proves zero
    // new readiness queries.
    let mut initial_readiness = harness
        .client
        .subscribe(subjects::worker_command_readiness_filter(PROVIDER, &worker))
        .await?;
    harness.client.flush().await?;
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let task = {
        let core = harness.core.clone();
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":record.lease_id,"command":["synthetic"],"detached":true,
            "stdin":"closed","require_launch_cancellation":true,"launch_key":"retained-key"
        }))?;
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    let initial = tokio::time::timeout(WAIT, initial_readiness.next())
        .await
        .context("the fresh launch must ask for readiness")?
        .context("the readiness subscription ended")?;
    let initial_request: CommandReadinessRequest = serde_json::from_slice(&initial.payload)?;
    assert_eq!(initial_request, Harness::readiness_binding(&record)?);
    assert_eq!(
        initial.subject.as_str(),
        subjects::worker_command_readiness(PROVIDER, &worker, &record.lease_id)
    );
    let initial_auth = initial
        .headers
        .as_ref()
        .and_then(|headers| headers.get(AUTH_HEADER))
        .context("the readiness query carries its signature")?;
    harness.verifier.verify_verb(
        initial.subject.as_str(),
        initial_auth.as_str(),
        &initial.payload,
        false,
        SystemTime::now(),
    )?;
    harness
        .client
        .publish(
            initial.reply.context("readiness carries a reply subject")?,
            Answer::Ready.response(&initial_request)?.into(),
        )
        .await?;
    initial_readiness.unsubscribe().await?;
    drop(initial_readiness);
    harness.client.flush().await?;
    let run = tokio::time::timeout(WAIT, runs.next())
        .await
        .context("the launch reaches its worker")?
        .context("the run subscription ended")?;
    let started: RunCommandResponse = serde_json::from_value(json!({
        "outcome":"started","execution_id":"retained-execution","origin":"created"
    }))?;
    harness
        .client
        .publish(
            run.reply.context("the run carries a reply subject")?,
            serde_json::to_vec(&started)?.into(),
        )
        .await?;
    assert_eq!(tokio::time::timeout(WAIT, task).await??, started);
    let mut readiness = harness
        .client
        .subscribe(subjects::worker_command_readiness_filter(PROVIDER, &worker))
        .await?;
    harness.client.flush().await?;
    harness.heartbeat(&worker, false).await?;
    let mut cancels = harness
        .client
        .subscribe(subjects::provider_cancel_command_launch(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let cancel_task = harness.core.handle_cancel_command_launch(CancelCommandLaunchRequest {
        lease_id: record.lease_id.clone(),
        launch_key: LaunchKey::new("retained-key")?,
    });
    tokio::pin!(cancel_task);
    let message = tokio::time::timeout(WAIT, async {
        tokio::select! {
            message = cancels.next() => message.context("the cancellation subscription ended"),
            response = &mut cancel_task => Err(format!("retained cancellation completed before reaching its exact worker: {response:?}").into()),
        }
    })
    .await
    .context("retained cancellation must reach its exact worker")??;
    let response = CancelCommandLaunchResponse::Canceled {
        execution_id: Some(ExecutionId::new("retained-execution")),
    };
    harness
        .client
        .publish(
            message.reply.context("cancellation carries a reply subject")?,
            serde_json::to_vec(&response)?.into(),
        )
        .await?;
    assert_eq!(tokio::time::timeout(WAIT, &mut cancel_task).await?, response);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), readiness.next())
            .await
            .is_err(),
        "retained cancellation must not issue a readiness query"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_ready_bindings_protocol_and_oversized_ready_mask_only_strong_features() -> TestResult {
    let cases = [
        ("tenant", Answer::ForeignReady(BindingField::Tenant)),
        ("provider", Answer::ForeignReady(BindingField::Provider)),
        ("lease", Answer::ForeignReady(BindingField::Lease)),
        ("native", Answer::ForeignReady(BindingField::Native)),
        ("worker", Answer::ForeignReady(BindingField::Worker)),
        ("attempt", Answer::ForeignReady(BindingField::Attempt)),
        ("epoch", Answer::ForeignReady(BindingField::Epoch)),
        ("user", Answer::ForeignReady(BindingField::User)),
        ("protocol", Answer::UnsupportedProtocol),
        ("oversized", Answer::OversizedReady),
    ];
    for (label, response) in cases {
        let sample = dummy_binding()?;
        let bytes = response.response(&sample)?;
        let decoded: CommandReadinessResponse = serde_json::from_slice(&bytes)?;
        match (response, decoded) {
            (Answer::ForeignReady(_), CommandReadinessResponse::Ready { binding, .. }) => {
                assert!(binding != sample, "the injected binding must actually differ");
            }
            (Answer::UnsupportedProtocol, CommandReadinessResponse::Ready { runtime, .. }) => {
                assert_ne!(runtime.protocol, CommandReadinessRuntime::PROTOCOL);
            }
            (Answer::OversizedReady, CommandReadinessResponse::Ready { .. }) => {
                assert!(bytes.len() > MAX_READY_BYTES);
            }
            _ => return Err("the fixture must inject a well-formed Ready response".into()),
        }
        let harness = Harness::new().await?;
        let worker = WorkerId::new(format!("readiness-binding-{label}"))?;
        let heartbeat = harness.heartbeat(&worker, true).await?;
        harness.answer(response).await;
        let (grant, _) = harness.lease(&worker, &format!("binding-{label}")).await?;
        assert!(harness.observed.lock().await.is_some());
        assert!(
            grant.capabilities == ordinary(&heartbeat.profile.sdk_features()),
            "a mismatched or over-bound Ready must retain ordinary features and mask strong features"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn individually_advertised_strong_flags_require_a_ready_reply() -> TestResult {
    for (label, command, launch) in [("command-only", true, false), ("launch-only", false, true)] {
        let harness = Harness::new().await?;
        let worker = WorkerId::new(format!("readiness-{label}"))?;
        let heartbeat = harness.heartbeat_flags(&worker, command, launch).await?;
        assert_eq!(
            heartbeat.profile.isolation.supports_command_cancellation,
            CapabilitySupport::from(command)
        );
        assert_eq!(
            heartbeat.profile.isolation.supports_command_launch_cancellation,
            CapabilitySupport::from(launch)
        );
        harness.answer(Answer::Denied).await;
        let (grant, _) = harness.lease(&worker, label).await?;
        assert!(
            harness.observed.lock().await.is_some(),
            "either individual strong flag must reach the signed readiness query"
        );
        assert!(
            grant.capabilities == ordinary(&heartbeat.profile.sdk_features()),
            "an individual strong flag must not bypass Ready or alter ordinary features"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_ready_reply_cannot_lend_authority_after_the_query_binding_changes() -> TestResult {
    for mutation in [
        QueryMutation::Tenant,
        QueryMutation::Provider,
        QueryMutation::Native,
        QueryMutation::Worker,
        QueryMutation::Attempt,
        QueryMutation::Epoch,
        QueryMutation::Expired,
        QueryMutation::Paused,
        QueryMutation::Freeze,
        QueryMutation::ProfileWithdrawal,
        QueryMutation::SameTimestampProfileWithdrawal,
        QueryMutation::ForeignProfile,
        QueryMutation::LegacyHeartbeat,
        QueryMutation::EpochSupportWithdrawal,
    ] {
        let harness = Harness::new().await?;
        let worker = WorkerId::new("readiness-changing-authority")?;
        let heartbeat = harness.heartbeat(&worker, true).await?;
        harness.answer(Answer::Ready).await;
        harness.mutate_during_query(mutation).await;
        let (grant, _) = harness.lease(&worker, "changing-authority").await?;
        assert_eq!(*harness.applied_mutation.lock().await, Some(mutation));
        assert!(harness.observed.lock().await.is_some());
        assert!(
            grant.capabilities == ordinary(&heartbeat.profile.sdk_features()),
            "an exact old Ready must not lend stale authority or remove ordinary features"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_equivalent_heartbeat_refresh_during_query_preserves_exact_ready_features() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("readiness-heartbeat-refresh")?;
    let heartbeat = harness.heartbeat(&worker, true).await?;
    harness.answer(Answer::Ready).await;
    harness.mutate_during_query(QueryMutation::HeartbeatRefresh).await;
    let (grant, _) = harness.lease(&worker, "heartbeat-refresh").await?;
    assert_eq!(
        *harness.applied_mutation.lock().await,
        Some(QueryMutation::HeartbeatRefresh)
    );
    assert!(
        grant.capabilities == all_features(&heartbeat.profile.sdk_features()),
        "an equivalent fresh heartbeat must retain both exact Ready features and ordinary features"
    );
    Ok(())
}
