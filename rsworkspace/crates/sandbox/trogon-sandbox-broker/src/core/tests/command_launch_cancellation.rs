use super::*;
use trogon_sandbox_contract::{
    CommandReadinessRequest, CommandReadinessTarget, LaunchCancellation, LeaseEpoch, WorkerDispatchProtocol,
};
use trogon_sandbox_protocol::{DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH};

struct StrongReadinessBus {
    commands: Arc<CountingWorkerBus>,
    verifier: trogon_sandbox_protocol::DispatchVerifier,
    binding: Option<trogon_sandbox_contract::CommandReadinessRequest>,
}

impl StrongReadinessBus {
    fn new(
        commands: Arc<CountingWorkerBus>,
        signer: &trogon_sandbox_protocol::DispatchSigner,
        record: &LeaseRecord,
    ) -> anyhow::Result<Self> {
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_epoch_mode(EpochMode::Require)
            .with_key(signer.key_id(), &signer.public_key_b64())?;
        let binding = match (
            record.worker_id.as_ref(),
            record.dispatch_attempt.as_ref(),
            record.epoch,
        ) {
            (Some(worker), Some(attempt), Some(epoch)) => {
                let target = trogon_sandbox_contract::DispatchTarget {
                    worker_id: worker.clone(),
                    attempt_id: attempt.id.clone(),
                    epoch: Some(epoch),
                };
                let payload = serde_json::to_vec(&serde_json::json!({
                    "lease_id":record.lease_id,"request":{"tenant":record.tenant},"target":target,
                }))?;
                let signed = signer.sign_payload(&payload, OP_LEASE_DISPATCH)?;
                verifier.decode::<serde_json::Value>(&signed, OP_LEASE_DISPATCH, Some(worker))?;
                Some(CommandReadinessRequest::new(
                    record.tenant.clone(),
                    record.lease_id.clone(),
                    record.provider,
                    CommandReadinessTarget {
                        worker_id: worker.clone(),
                        attempt_id: attempt.id.clone(),
                        epoch,
                    },
                    SandboxId::new(record.sandbox_id.clone())?,
                    None,
                )?)
            }
            _ => None,
        };
        Ok(Self {
            commands,
            verifier,
            binding,
        })
    }
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for StrongReadinessBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        anyhow::ensure!(
            !subject.contains(".command_readiness."),
            "fixture readiness query must be signed"
        );
        crate::bus::BrokerBus::request(self.commands.as_ref(), subject, payload).await
    }

    async fn request_with_timeout(&self, subject: &str, payload: Bytes, timeout: Duration) -> anyhow::Result<Bytes> {
        anyhow::ensure!(
            !subject.contains(".command_readiness."),
            "fixture readiness query must be signed"
        );
        crate::bus::BrokerBus::request_with_timeout(self.commands.as_ref(), subject, payload, timeout).await
    }

    async fn request_with_auth(
        &self,
        subject: &str,
        auth: &str,
        payload: Bytes,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Bytes> {
        if subject.contains(".command_readiness.") {
            let binding = self
                .binding
                .as_ref()
                .context("fixture has no assigned readiness binding")?;
            let expected_subject = trogon_sandbox_protocol::subjects::worker_command_readiness(
                binding.provider(),
                &binding.target().worker_id,
                binding.lease_id(),
            );
            anyhow::ensure!(
                subject == expected_subject,
                "fixture readiness must address its exact worker and lease"
            );
            let verified = self
                .verifier
                .verify_verb_scope(subject, auth, &payload, false, SystemTime::now())?;
            anyhow::ensure!(
                verified.matches(subject, &payload),
                "fixture readiness signature lost its exact bytes"
            );
            let scope = verified.scope();
            anyhow::ensure!(
                scope.tenant == *binding.tenant()
                    && scope.lease_id == *binding.lease_id()
                    && scope.worker_id.as_ref() == Some(&binding.target().worker_id)
                    && scope.attempt_id.as_ref() == Some(&binding.target().attempt_id)
                    && scope.epoch == Some(binding.target().epoch),
                "fixture readiness signed scope differs from its held assignment"
            );
            let request: trogon_sandbox_contract::CommandReadinessRequest = serde_json::from_slice(&payload)?;
            anyhow::ensure!(
                request == *binding,
                "fixture readiness request differs from its exact held user/native binding"
            );
            let response = trogon_sandbox_contract::CommandReadinessResponse::Ready {
                binding: request,
                runtime: trogon_sandbox_contract::CommandReadinessRuntime {
                    sha256: trogon_sandbox_contract::Digest::new(format!("sha256:{}", "a".repeat(64)))?,
                    protocol: trogon_sandbox_contract::CommandReadinessRuntime::PROTOCOL,
                },
            };
            return Ok(serde_json::to_vec(&response)?.into());
        }
        crate::bus::BrokerBus::request_with_timeout(
            self.commands.as_ref(),
            subject,
            payload,
            timeout.unwrap_or(Duration::from_secs(10)),
        )
        .await
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        crate::bus::BrokerBus::publish(self.commands.as_ref(), subject, payload).await
    }
}

async fn strong_launch_fixture(
    assigned: Option<&str>,
    profiles: &[(&str, bool, bool)],
) -> anyhow::Result<(BrokerCore, Arc<CountingWorkerBus>, LeaseId)> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    for (worker, command, launch) in profiles {
        let mut profile = default_capability_profile();
        profile.isolation.supports_command_cancellation = CapabilitySupport::from(*command);
        profile.isolation.supports_command_launch_cancellation = CapabilitySupport::from(*launch);
        let mut heartbeat = worker_heartbeat(worker, ProviderKind::Mock, profile)?;
        heartbeat.supports_lease_epoch = CapabilitySupport::Supported;
        heartbeat.capacity.current_leases = 4;
        heartbeat.dispatch_protocol = WorkerDispatchProtocol::TargetedV1;
        registry.inject(heartbeat).await;
    }
    let bus = Arc::new(CountingWorkerBus::new(completed_with_stdout("effect"), Duration::ZERO));
    let lease = LeaseId::new("strong-launch-lease");
    let mut record = running_source_record(&TenantId::new("strong-launch-tenant")?, &lease)?;
    record.sandbox_id = "strong-launch-sandbox".into();
    record.worker_id = assigned.map(WorkerId::new).transpose()?;
    if assigned.is_some() {
        record.epoch = Some(LeaseEpoch::FIRST);
        record.dispatch_attempt = Some(serde_json::from_value(serde_json::json!({
            "id":"strong-launch-attempt", "epoch":1,
            "idempotency_key":"strong-launch-request", "accounting":"tenant"
        }))?);
    }
    let signer = Arc::new(trogon_sandbox_protocol::DispatchSigner::new(
        "strong-readiness-fixture",
        [53; 32],
    ));
    let mut core = make_core_with_bus(Arc::new(StrongReadinessBus::new(bus.clone(), &signer, &record)?))?
        .with_dispatch_signer(signer);
    core.worker_registry = registry;
    core.config.enforce_worker_registry = false;
    core.active_leases.put(&record).await?;
    Ok((core, bus, lease))
}

fn strong_command(lease: &LeaseId, key: Option<&str>) -> anyhow::Result<RunCommandRequest> {
    let mut request = base_run_request(lease.clone(), &["effect"]);
    request.execution = detached(Default::default());
    request.require_launch_cancellation = LaunchCancellation::Required;
    request.launch_key = key.map(trogon_sandbox_contract::LaunchKey::new).transpose()?;
    Ok(request)
}

#[tokio::test]
async fn a_full_exact_assigned_worker_can_serve_its_existing_leases_strong_command() -> TestResult {
    let (core, bus, lease) = strong_launch_fixture(Some("assigned"), &[("assigned", true, true)]).await?;
    let response = core.handle_run_command(strong_command(&lease, Some("owned"))?).await;
    assert!(
        matches!(response, RunCommandResponse::Completed(_)),
        "a full worker must retain command controls for its existing lease: {response:?}"
    );
    assert_eq!(
        bus.dispatches(),
        1,
        "the exact assigned worker must receive the authorized command once"
    );
    Ok(())
}

#[tokio::test]
async fn a_strong_launch_cannot_borrow_another_workers_capability() -> TestResult {
    let (core, bus, lease) =
        strong_launch_fixture(Some("assigned"), &[("assigned", true, false), ("other", true, true)]).await?;
    let response = core.handle_run_command(strong_command(&lease, Some("owned"))?).await;
    assert!(
        matches!(response, RunCommandResponse::Denied(_)),
        "an unearned assigned-worker capability must refuse before any effect: {response:?}"
    );
    assert_eq!(
        bus.dispatches(),
        0,
        "another worker's profile must not authorize this lease"
    );
    Ok(())
}

#[tokio::test]
async fn a_strong_launch_without_current_assignment_is_refused_even_with_registry_unenforced() -> TestResult {
    let (core, bus, lease) = strong_launch_fixture(None, &[("other", true, true)]).await?;
    let response = core.handle_run_command(strong_command(&lease, Some("owned"))?).await;
    assert!(
        matches!(response, RunCommandResponse::Denied(_)),
        "legacy assignment metadata cannot establish cancellation custody: {response:?}"
    );
    assert_eq!(
        bus.dispatches(),
        0,
        "missing assignment authority must refuse before dispatch"
    );
    Ok(())
}

#[tokio::test]
async fn a_strong_launch_requires_its_own_client_retained_launch_key() -> TestResult {
    let (core, bus, lease) = strong_launch_fixture(Some("assigned"), &[("assigned", true, true)]).await?;
    let response = core.handle_run_command(strong_command(&lease, None)?).await;
    assert!(
        matches!(response, RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })),
        "a launch without a pre-ack reference cannot promise its cancellation: {response:?}"
    );
    assert_eq!(
        bus.dispatches(),
        0,
        "missing launch identity must refuse before dispatch"
    );
    Ok(())
}

#[tokio::test]
async fn launch_cancellation_requires_the_underlying_exact_command_cancellation() -> TestResult {
    let (core, bus, lease) = strong_launch_fixture(Some("assigned"), &[("assigned", false, true)]).await?;
    let response = core.handle_run_command(strong_command(&lease, Some("owned"))?).await;
    assert!(
        matches!(response, RunCommandResponse::Denied(_)),
        "a stronger feature cannot be earned without native exact stop: {response:?}"
    );
    assert_eq!(
        bus.dispatches(),
        0,
        "a contradictory worker profile must refuse before dispatch"
    );
    Ok(())
}
