//! Broker component contracts over real NATS with synthetic assigned workers.
//! These tests establish routing and feature projection, not native cancellation.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::{Client, Message, Subscriber};
use futures::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_broker::active_lease::{ActiveLeaseStore, InMemoryActiveLeaseStore};
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
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CapabilitySupport, CommandReadinessRequest,
    CommandReadinessResponse, CommandReadinessRuntime, CommandReadinessTarget, Digest, DispatchTarget, ExecutionId,
    IsolationTier, LaunchKey, LeaseEpoch, LeaseError, LeaseGranted, LeaseId, LeaseRecord, LeaseResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, SandboxId, TenantId,
    WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH};

#[path = "assigned_cancellation_contract/context.rs"]
mod context;
use context::{Context, TestResult};

const WAIT: Duration = Duration::from_secs(10);
const PROVIDER: ProviderKind = ProviderKind::LocalDocker;
const SIGNING_SEED_BYTE: u8 = 37;

struct Harness {
    core: Arc<BrokerCore>,
    active: Arc<dyn ActiveLeaseStore>,
    registry: Arc<NatsWorkerRegistry>,
    verifier: DispatchVerifier,
    client: Client,
    cancel: CancellationToken,
    server: JetStreamTestServer,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn assigned_worker(record: &LeaseRecord) -> TestResult<&WorkerId> {
    record
        .worker_id
        .as_ref()
        .context("the lease record names no assigned worker")
}

fn readiness_target(target: &DispatchTarget) -> TestResult<CommandReadinessTarget> {
    Ok(CommandReadinessTarget {
        worker_id: target.worker_id.clone(),
        attempt_id: target.attempt_id.clone(),
        epoch: target.epoch.context("an assigned dispatch target carries its epoch")?,
    })
}

impl Harness {
    async fn new() -> TestResult<Self> {
        Self::with_launch_store(None).await
    }

    async fn with_launch_store(
        launches: Option<Arc<dyn trogon_sandbox_broker::command_launch::CommandLaunchStore>>,
    ) -> TestResult<Self> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let cancel = CancellationToken::new();
        let registry = NatsWorkerRegistry::new(client.clone(), Duration::from_secs(60), cancel.clone()).await?;
        client.flush().await?;
        let active: Arc<dyn ActiveLeaseStore> = Arc::new(InMemoryActiveLeaseStore::new());
        let signer = Arc::new(DispatchSigner::new(
            "component-contract",
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
        let core = Arc::new(match launches {
            Some(launches) => core.with_command_launch_store(launches),
            None => core,
        });
        Ok(Self {
            core,
            active,
            registry,
            verifier,
            client,
            cancel,
            server,
        })
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
                    .is_some_and(|seen| {
                        seen.emitted_at == heartbeat.emitted_at
                            && seen.profile == heartbeat.profile
                            && seen.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
                            && seen.supports_lease_epoch.is_supported()
                    })
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

    async fn begin_lease(&self, worker: &WorkerId, key: &str) -> TestResult<PendingLease> {
        let mut selected = self
            .client
            .subscribe(subjects::worker_dispatch(PROVIDER, worker))
            .await?;
        let mut legacy = self.client.subscribe(subjects::provider_dispatch(PROVIDER)).await?;
        self.client.flush().await?;
        let request: trogon_sandbox_contract::LeaseRequest = serde_json::from_value(json!({
            "tenant":"component-tenant", "provider_pref":["local_docker"],
            "spec":spec()?,
            "ttl_seconds":60, "idempotency_key":key, "persistent":false,
        }))?;
        let core = self.core.clone();
        let response = tokio::spawn(async move { core.handle_lease_request(request).await });
        let message = tokio::time::timeout(WAIT, async {
            tokio::select! {
                message = selected.next() => message.context("selected worker subscription ended"),
                message = legacy.next() => Err(format!("assigned selection used legacy dispatch: {:?}", message.map(|msg| msg.subject)).into()),
            }
        })
        .await
        .context("the broker must dispatch to its actual selected worker")??;
        let payload: ProviderDispatchPayload = self
            .verifier
            .decode(&message.payload, OP_LEASE_DISPATCH, Some(worker))
            .context("actual assigned dispatch must have valid scoped signature")?;
        let target = payload.target.as_ref().context("assigned dispatch target")?;
        assert_eq!(&target.worker_id, worker);
        assert_eq!(target.epoch, Some(LeaseEpoch::FIRST));
        assert_eq!(message.subject.as_str(), subjects::worker_dispatch(PROVIDER, worker));
        Ok(PendingLease {
            message,
            payload,
            response,
            legacy,
        })
    }

    async fn finish_lease(&self, pending: PendingLease) -> TestResult<(LeaseGranted, LeaseRecord)> {
        let target = pending.payload.target.as_ref().context("assigned dispatch target")?;
        let receipt = ProviderLease {
            sandbox_id: SandboxId::new(format!("component-stub-{}", pending.payload.lease_id))?,
            evidence_subject: subjects::evidence_subject(&pending.payload.lease_id),
            worker_id: Some(target.worker_id.clone()),
            attempt_id: Some(target.attempt_id.clone()),
            epoch: target.epoch,
        };
        let binding = CommandReadinessRequest::new(
            pending.payload.request.tenant.clone(),
            pending.payload.lease_id.clone(),
            PROVIDER,
            readiness_target(target)?,
            receipt.sandbox_id.clone(),
            None,
        )?;
        let readiness = if self
            .registry
            .current_worker(PROVIDER, &target.worker_id)
            .await
            .is_some_and(|heartbeat| {
                heartbeat.profile.isolation.supports_command_cancellation.is_supported()
                    || heartbeat
                        .profile
                        .isolation
                        .supports_command_launch_cancellation
                        .is_supported()
            }) {
            Some(self.subscribe_readiness(&binding).await?)
        } else {
            None
        };
        self.client
            .publish(
                pending.message.reply.context("the dispatch carries a reply subject")?,
                serde_json::to_vec(&receipt)?.into(),
            )
            .await?;
        if let Some(readiness) = readiness {
            self.complete_readiness(readiness, &binding).await?;
        }
        let response = tokio::time::timeout(WAIT, pending.response)
            .await
            .context("actual selected worker reply must settle")??;
        let LeaseResponse::Granted(grant) = response else {
            return Err(format!("valid selected worker receipt must grant: {response:?}").into());
        };
        let record = self
            .active
            .get(&grant.lease_id)
            .await?
            .context("the granted lease must be stored")?;
        assert_eq!(record.worker_id, receipt.worker_id);
        assert_eq!(
            record.dispatch_attempt.as_ref().map(|attempt| &attempt.id),
            receipt.attempt_id.as_ref()
        );
        assert_eq!(record.epoch, receipt.epoch);
        assert_eq!(record.lease_id, pending.payload.lease_id);
        assert_eq!(grant.sandbox_id, receipt.sandbox_id.as_str());
        assert_eq!(record.sandbox_id, receipt.sandbox_id.as_str());
        let mut legacy = pending.legacy;
        no_message(&mut legacy, "assigned dispatch must not fall back to legacy").await;
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
            record.provider,
            CommandReadinessTarget {
                worker_id: assigned_worker(record)?.clone(),
                attempt_id: attempt.id.clone(),
                epoch: record.epoch.context("the lease record names its epoch")?,
            },
            SandboxId::new(record.sandbox_id.clone())?,
            None,
        )?)
    }

    async fn subscribe_readiness(&self, binding: &CommandReadinessRequest) -> TestResult<Subscriber> {
        let subscription = self
            .client
            .subscribe(subjects::worker_command_readiness(
                binding.provider(),
                &binding.target().worker_id,
                binding.lease_id(),
            ))
            .await?;
        self.client.flush().await?;
        Ok(subscription)
    }

    async fn reply_readiness(&self, message: Message, expected: &CommandReadinessRequest) -> TestResult {
        assert!(message.payload.len() <= CommandReadinessRequest::MAX_BYTES);
        assert_eq!(
            message.subject.as_str(),
            subjects::worker_command_readiness(expected.provider(), &expected.target().worker_id, expected.lease_id())
        );
        let verified = self
            .verifier
            .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)
            .context("component readiness requires a valid signed envelope")?
            .context("component readiness must retain authenticated authority")?;
        assert!(verified.matches(message.subject.as_str(), &message.payload));
        let scope = verified.scope();
        assert_eq!(&scope.tenant, expected.tenant());
        assert_eq!(&scope.lease_id, expected.lease_id());
        assert_eq!(scope.worker_id.as_ref(), Some(&expected.target().worker_id));
        assert_eq!(scope.attempt_id.as_ref(), Some(&expected.target().attempt_id));
        assert_eq!(scope.epoch, Some(expected.target().epoch));
        let request: CommandReadinessRequest = serde_json::from_slice(&message.payload)?;
        assert_eq!(
            &request, expected,
            "component readiness cannot lend a foreign held user/native binding"
        );
        let response = CommandReadinessResponse::Ready {
            binding: request,
            runtime: CommandReadinessRuntime {
                sha256: Digest::new(format!("sha256:{}", "a".repeat(64)))?,
                protocol: CommandReadinessRuntime::PROTOCOL,
            },
        };
        self.client
            .publish(
                message.reply.context("readiness query requires a reply")?,
                serde_json::to_vec(&response)?.into(),
            )
            .await?;
        self.client.flush().await?;
        Ok(())
    }

    async fn complete_readiness(&self, mut subscription: Subscriber, expected: &CommandReadinessRequest) -> TestResult {
        let message = tokio::time::timeout(WAIT, subscription.next())
            .await
            .context("positive setup requires one exact readiness query")?
            .context("readiness subscription ended")?;
        self.reply_readiness(message, expected).await?;
        subscription.unsubscribe().await?;
        self.client.flush().await?;
        Ok(())
    }

    fn verify_owned_verb(&self, message: &Message, record: &LeaseRecord) -> TestResult {
        let verified = self
            .verifier
            .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)
            .context("actual broker verb must verify under Require and required epoch")?
            .context("actual broker verb must retain signed scope")?;
        let scope = verified.scope();
        assert_eq!(scope.tenant, record.tenant);
        assert_eq!(scope.lease_id, record.lease_id);
        assert_eq!(scope.worker_id, record.worker_id);
        assert_eq!(
            scope.attempt_id.as_ref(),
            record.dispatch_attempt.as_ref().map(|attempt| &attempt.id)
        );
        assert_eq!(scope.epoch, record.epoch);
        Ok(())
    }

    async fn cancel(
        &self,
        request: CancelCommandLaunchRequest,
        record: &LeaseRecord,
        worker: &mut Subscriber,
        answer: &CancelCommandLaunchResponse,
    ) -> TestResult<(CancelCommandLaunchResponse, bool)> {
        let response = self.core.handle_cancel_command_launch(request.clone());
        tokio::pin!(response);
        let message = tokio::time::timeout(WAIT, async {
            tokio::select! {
                reply = &mut response => Err(reply),
                message = worker.next() => Ok(message),
            }
        })
        .await
        .context("broker cancellation must respond or reach its retained worker")?;
        let message = match message {
            Ok(message) => message.context("owned cancellation subscription ended")?,
            Err(response) => return Ok((response, false)),
        };
        self.verify_owned_verb(&message, record)?;
        assert_eq!(
            message.subject.as_str(),
            subjects::provider_cancel_command_launch(PROVIDER, &record.lease_id)
        );
        let observed: CancelCommandLaunchRequest = serde_json::from_slice(&message.payload)?;
        assert_eq!(observed, request);
        self.client
            .publish(
                message.reply.context("cancellation carries a reply subject")?,
                serde_json::to_vec(answer)?.into(),
            )
            .await?;
        Ok((
            tokio::time::timeout(WAIT, &mut response)
                .await
                .context("the broker settles the cancellation")?,
            true,
        ))
    }
}

struct PendingLease {
    message: Message,
    payload: ProviderDispatchPayload,
    response: tokio::task::JoinHandle<LeaseResponse>,
    legacy: Subscriber,
}

fn spec() -> TestResult<trogon_sandbox_contract::SandboxSpec> {
    Ok(trogon_sandbox_contract::SandboxSpec::new(
        trogon_sandbox_contract::SandboxSpecParts {
            image: trogon_sandbox_contract::SandboxImage::Registry {
                reference: trogon_sandbox_contract::OciImageRef::parse_unqualified("base"),
                pull_secret: None,
            },
            workspace: trogon_sandbox_contract::WorkspaceSource::empty(),
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
        },
    )?)
}

async fn no_message(subscription: &mut Subscriber, reason: &str) {
    assert!(
        tokio::time::timeout(Duration::from_millis(100), subscription.next())
            .await
            .is_err(),
        "{reason}"
    );
}

async fn mixed_pool_grant(selected_strong: bool, later_strong: bool) -> TestResult {
    let harness = Harness::new().await?;
    let selected = WorkerId::new("selected-worker")?;
    let other = WorkerId::new("later-worker")?;
    let selected_heartbeat = harness.heartbeat(&selected, selected_strong).await?;
    let mut unrelated_dispatch = harness
        .client
        .subscribe(subjects::worker_dispatch(PROVIDER, &other))
        .await?;
    harness.client.flush().await?;
    let pending = harness.begin_lease(&selected, "mixed-pool-grant").await?;
    let later_heartbeat = harness.heartbeat(&other, later_strong).await?;
    assert!(later_heartbeat.emitted_at > selected_heartbeat.emitted_at);
    let latest = harness
        .registry
        .profile_for_provider(PROVIDER)
        .await
        .context("the registry knows a profile for the provider")?;
    assert_eq!(latest, later_heartbeat.profile);
    let (grant, record) = harness.finish_lease(pending).await?;
    no_message(
        &mut unrelated_dispatch,
        "the later heartbeat must not replace the selected assignment",
    )
    .await;
    eprintln!(
        "broker mixed-pool projection receipt: selected_worker={} later_worker={} actual_record_worker={} selected_strong={selected_strong} later_strong={later_strong} grant_features={:?} worker_is_stub=true native_acceptance=false",
        selected,
        other,
        assigned_worker(&record)?,
        grant.capabilities
    );
    let expected: Vec<String> = selected_heartbeat
        .profile
        .sdk_features()
        .into_iter()
        .map(|feature| feature.as_str().to_owned())
        .collect();
    assert_eq!(
        grant.capabilities, expected,
        "SDK grant features must describe the actual selected worker, even when another same-provider heartbeat is newer"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_assigned_grant_cannot_borrow_a_later_same_provider_workers_strong_features() -> TestResult {
    mixed_pool_grant(false, true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_assigned_grants_strong_features_survive_a_later_ineligible_workers_heartbeat() -> TestResult {
    mixed_pool_grant(true, false).await
}

fn command(lease: &LeaseId, key: &str) -> TestResult<RunCommandRequest> {
    Ok(serde_json::from_value(json!({
        "lease_id":lease, "command":["component-stub-command"], "detached":true,
        "stdin":"closed", "deadline":"lease", "launch_key":key,
        "require_launch_cancellation":true,
    }))?)
}

fn cancellation(lease: &LeaseId, key: &str) -> TestResult<CancelCommandLaunchRequest> {
    Ok(CancelCommandLaunchRequest {
        lease_id: lease.clone(),
        launch_key: LaunchKey::new(key)?,
    })
}

async fn retained_cancellation_after_withdrawal(start_reply_pending: bool) -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("retained-owner")?;
    harness.heartbeat(&worker, true).await?;
    let pending = harness.begin_lease(&worker, "retained-launch-lease").await?;
    let (grant, record) = harness.finish_lease(pending).await?;
    assert!(grant.capabilities.contains(&"command_cancellation".into()));
    assert!(grant.capabilities.contains(&"command_launch_cancellation".into()));
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    let mut cancels = harness
        .client
        .subscribe(subjects::provider_cancel_command_launch(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;

    let healthy_response = CancelCommandLaunchResponse::Canceled {
        execution_id: Some(ExecutionId::new("component-healthy-control")),
    };
    let unknown_response = CancelCommandLaunchResponse::Denied(LeaseError::InvalidSpec {
        reason: "the synthetic worker holds no matching launch".into(),
    });
    let (unknown, unknown_routed) = harness
        .cancel(
            cancellation(&record.lease_id, "unknown-launch")?,
            &record,
            &mut cancels,
            &unknown_response,
        )
        .await?;
    assert_eq!(unknown, unknown_response);
    assert!(unknown_routed);

    let healthy = command(&record.lease_id, "healthy-launch")?;
    let readiness = harness
        .subscribe_readiness(&Harness::readiness_binding(&record)?)
        .await?;
    let healthy_task = {
        let core = harness.core.clone();
        tokio::spawn(async move { core.handle_run_command(healthy).await })
    };
    harness
        .complete_readiness(readiness, &Harness::readiness_binding(&record)?)
        .await?;
    let healthy_run = tokio::time::timeout(WAIT, runs.next())
        .await
        .context("the healthy launch reaches its worker")?
        .context("the run subscription ended")?;
    harness.verify_owned_verb(&healthy_run, &record)?;
    let healthy_observed: RunCommandRequest = serde_json::from_slice(&healthy_run.payload)?;
    assert_eq!(
        healthy_observed.launch_key.as_ref().map(LaunchKey::as_str),
        Some("healthy-launch")
    );
    let healthy_started: RunCommandResponse = serde_json::from_value(json!({
        "outcome":"started", "execution_id":"component-healthy-control", "origin":"created",
    }))?;
    harness
        .client
        .publish(
            healthy_run.reply.context("the run carries a reply subject")?,
            serde_json::to_vec(&healthy_started)?.into(),
        )
        .await?;
    assert_eq!(
        tokio::time::timeout(WAIT, healthy_task)
            .await
            .context("the healthy launch settles")??,
        healthy_started
    );
    let (healthy_canceled, healthy_routed) = harness
        .cancel(
            cancellation(&record.lease_id, "healthy-launch")?,
            &record,
            &mut cancels,
            &healthy_response,
        )
        .await?;
    assert_eq!(healthy_canceled, healthy_response);
    assert!(
        healthy_routed,
        "healthy exact cancellation is a required positive control"
    );

    let old = command(&record.lease_id, "retained-launch")?;
    let readiness = harness
        .subscribe_readiness(&Harness::readiness_binding(&record)?)
        .await?;
    let old_task = {
        let core = harness.core.clone();
        let request = old.clone();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    harness
        .complete_readiness(readiness, &Harness::readiness_binding(&record)?)
        .await?;
    let old_message = tokio::time::timeout(WAIT, runs.next())
        .await
        .context("the retained launch reaches its worker")?
        .context("the run subscription ended")?;
    harness.verify_owned_verb(&old_message, &record)?;
    let old_observed: RunCommandRequest = serde_json::from_slice(&old_message.payload)?;
    assert_eq!(old_observed.launch_key, old.launch_key);
    assert_eq!(old_observed.command, old.command);
    assert!(
        old_observed.require_launch_cancellation.is_required()
            && matches!(
                old_observed.execution,
                trogon_sandbox_contract::CommandExecution::Detached { .. }
            )
    );
    let old_started: RunCommandResponse = serde_json::from_value(json!({
        "outcome":"started", "execution_id":"component-retained-execution", "origin":"created",
    }))?;
    let old_reply = old_message.reply.context("the run carries a reply subject")?;
    let mut old_task = Some(old_task);
    if !start_reply_pending {
        harness
            .client
            .publish(old_reply.clone(), serde_json::to_vec(&old_started)?.into())
            .await?;
        assert_eq!(
            tokio::time::timeout(WAIT, old_task.take().context("the old launch task is pending")?)
                .await
                .context("the old launch settles")??,
            old_started
        );
    } else {
        assert!(
            !old_task
                .as_ref()
                .context("the old launch task is pending")?
                .is_finished()
        );
    }

    harness.heartbeat(&worker, false).await?;
    let unchanged = harness
        .active
        .get(&record.lease_id)
        .await?
        .context("the lease stays stored")?;
    assert_eq!(unchanged.worker_id, record.worker_id);
    assert_eq!(unchanged.dispatch_attempt, record.dispatch_attempt);
    assert_eq!(unchanged.epoch, record.epoch);
    assert_eq!(unchanged.sandbox_id, record.sandbox_id);
    let new_start = harness
        .core
        .handle_run_command(command(&record.lease_id, "new-after-withdrawal")?)
        .await;
    assert!(matches!(new_start, RunCommandResponse::Denied(_)));
    no_message(&mut runs, "withdrawn admission must prevent a new strong effect").await;

    let (unknown_after_withdrawal, unknown_routed_after_withdrawal) = harness
        .cancel(
            cancellation(&record.lease_id, "never-admitted-launch")?,
            &record,
            &mut cancels,
            &unknown_response,
        )
        .await?;
    assert!(matches!(
        unknown_after_withdrawal,
        CancelCommandLaunchResponse::Unsupported | CancelCommandLaunchResponse::Denied(_)
    ));
    assert!(
        !unknown_routed_after_withdrawal,
        "withdrawn admission must not invent retained cancellation authority for an unknown launch"
    );

    let unknown_lease = harness
        .core
        .handle_cancel_command_launch(cancellation(&LeaseId::new("unknown-lease"), "retained-launch")?)
        .await;
    assert!(matches!(
        unknown_lease,
        CancelCommandLaunchResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    let mut no_assignment = record.clone();
    no_assignment.lease_id = LeaseId::new("legacy-no-assignment");
    no_assignment.worker_id = None;
    no_assignment.dispatch_attempt = None;
    no_assignment.epoch = None;
    harness.active.put(&no_assignment).await?;
    let no_authority = harness
        .core
        .handle_cancel_command_launch(cancellation(&no_assignment.lease_id, "retained-launch")?)
        .await;
    assert_eq!(no_authority, CancelCommandLaunchResponse::Unsupported);
    no_message(
        &mut cancels,
        "unknown and unassigned leases must not borrow retained authority",
    )
    .await;

    let expected = CancelCommandLaunchResponse::Canceled {
        execution_id: Some(ExecutionId::new("component-retained-execution")),
    };
    let (canceled, routed) = harness
        .cancel(
            cancellation(&record.lease_id, "retained-launch")?,
            &record,
            &mut cancels,
            &expected,
        )
        .await?;
    let (replayed, replay_routed) = harness
        .cancel(
            cancellation(&record.lease_id, "retained-launch")?,
            &record,
            &mut cancels,
            &expected,
        )
        .await?;
    if let Some(task) = old_task {
        harness
            .client
            .publish(old_reply, serde_json::to_vec(&old_started)?.into())
            .await?;
        assert_eq!(
            tokio::time::timeout(WAIT, task)
                .await
                .context("the pending old launch settles")??,
            old_started
        );
    }
    no_message(
        &mut runs,
        "old cancellation and replay must never dispatch another start",
    )
    .await;
    eprintln!(
        "broker withdrawn-admission receipt: old_start_observed=true old_reply_pending={start_reply_pending} healthy_cancel_routed=true new_strong_start_refused=true assignment_unchanged=true old_cancel_routed={routed} old_replay_routed={replay_routed} old_cancel={canceled:?} old_replay={replayed:?} worker_is_stub=true native_acceptance=false"
    );
    assert_eq!(
        canceled, expected,
        "withdrawing new strong admission must preserve cancellation of an exact already-owned launch"
    );
    assert!(routed, "old cancellation must reach the retained scoped owner");
    assert_eq!(
        replayed, expected,
        "old cancellation replay must retain its exact outcome"
    );
    assert!(replay_routed);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawing_new_admission_preserves_exact_started_launch_cancellation() -> TestResult {
    retained_cancellation_after_withdrawal(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawing_new_admission_preserves_exact_pending_launch_cancellation() -> TestResult {
    retained_cancellation_after_withdrawal(true).await
}

#[path = "assigned_cancellation_contract/retained_authority.rs"]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod retained_authority;

#[path = "assigned_cancellation_contract/authority_store.rs"]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod authority_store;
