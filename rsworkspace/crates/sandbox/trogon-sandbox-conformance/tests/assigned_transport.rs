//! An assigned worker grants only signed dispatches that name it and honours only controls signed for the exact
//! worker, attempt and epoch of a live grant. The suite reaches such a worker and releases every grant it took.

mod support;

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_nats::{Client, HeaderMap, Message};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use support::wire::{self, Sendable};
use support::{heartbeat, locked};
use tokio::task::JoinHandle;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{GrantLedger, Target, checks_for_target};
use trogon_sandbox_contract::{
    CancelIntent, CapabilitySupport, CommandOutput, CommandTimeoutStatus, DispatchAttemptId, DispatchTarget,
    LeaseEpoch, LeaseId, OutputTruncation, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload,
    ProviderKind, ProviderLease, RunCommandResponse, SandboxId, TenantId, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkerId,
};
use trogon_sandbox_protocol::subjects::{
    provider_cancel, worker_cleanup, worker_dispatch, worker_heartbeat, worker_heartbeat_filter,
};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH, VerbScope,
};

const KIND: ProviderKind = ProviderKind::Mock;

/// The tenant the suite leases for.
const TENANT: &str = "conformance";

#[derive(Default)]
struct Observed {
    grants: Vec<(LeaseId, ProviderLease)>,
    controls: Vec<VerbScope>,
    live: HashMap<String, ProviderLease>,
    refused_controls: usize,
}

/// Whether `scope` names exactly the worker, attempt and epoch of the live grant for its lease.
fn binds_live_grant(observed: &Mutex<Observed>, scope: &VerbScope) -> bool {
    let state = locked(observed);
    state.live.get(scope.lease_id.as_str()).is_some_and(|native| {
        scope.tenant.as_str() == TENANT
            && scope.worker_id == native.worker_id
            && scope.attempt_id == native.attempt_id
            && scope.epoch == native.epoch
    })
}

struct AssignedWorker {
    client: Client,
    worker_id: WorkerId,
    signer: Arc<DispatchSigner>,
    observed: Arc<Mutex<Observed>>,
    task: JoinHandle<Sendable<()>>,
}

impl Drop for AssignedWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Serving {
    client: Client,
    worker_id: WorkerId,
    verifier: DispatchVerifier,
    observed: Arc<Mutex<Observed>>,
}

impl Serving {
    async fn heartbeat(&self) -> Sendable<()> {
        let heartbeat = heartbeat(
            KIND,
            &self.worker_id,
            WorkerDispatchProtocol::TargetedV1,
            CapabilitySupport::Supported,
        );
        self.client
            .publish(
                worker_heartbeat(KIND, &self.worker_id),
                serde_json::to_vec(&heartbeat)?.into(),
            )
            .await?;
        Ok(())
    }

    async fn dispatch(&self, message: Message) -> Sendable<()> {
        let request: ProviderDispatchPayload =
            self.verifier
                .decode(&message.payload, OP_LEASE_DISPATCH, Some(&self.worker_id))?;
        let target = request.target.ok_or("an assigned dispatch carries its target")?;
        if target.worker_id != self.worker_id || target.epoch != Some(LeaseEpoch::FIRST) {
            return Err("the dispatch names another worker or epoch".into());
        }
        let native = ProviderLease {
            sandbox_id: SandboxId::new(format!("native-{}", request.lease_id.as_str()))?,
            evidence_subject: format!("sandbox.evidence.{}", request.lease_id.as_str()),
            worker_id: Some(target.worker_id),
            attempt_id: Some(target.attempt_id),
            epoch: target.epoch,
        };
        {
            let mut state = locked(&self.observed);
            state.grants.push((request.lease_id.clone(), native.clone()));
            state.live.insert(request.lease_id.as_str().to_owned(), native.clone());
        }
        reply(&self.client, &message, &native).await
    }

    async fn cleanup(&self, message: Message) -> Sendable<()> {
        let request: ProviderCleanupRequest = serde_json::from_slice(&message.payload)?;
        let identity = &request.identity;
        let admitted = self
            .verifier
            .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)
            .ok()
            .flatten()
            .filter(|verified| verified.matches(message.subject.as_str(), &message.payload))
            .map(|verified| verified.scope().clone())
            .filter(|scope| {
                scope.lease_id == identity.lease_id
                    && identity.provider == KIND
                    && identity.target.worker_id == self.worker_id
                    && scope.worker_id.as_ref() == Some(&identity.target.worker_id)
                    && scope.attempt_id.as_ref() == Some(&identity.target.attempt_id)
                    && scope.epoch == identity.target.epoch
                    && binds_live_grant(&self.observed, scope)
            });
        let Some(scope) = admitted else {
            locked(&self.observed).refused_controls += 1;
            let refused = ProviderCleanupResponse::Failed {
                identity: request.identity,
                message: "assignment scope mismatch".to_owned(),
            };
            return reply(&self.client, &message, &refused).await;
        };
        if request.intent != CancelIntent::Release {
            return Err("the suite releases its grants, it does not purge them".into());
        }
        {
            let mut state = locked(&self.observed);
            state.live.remove(scope.lease_id.as_str());
            state.controls.push(scope);
        }
        let cleaned = ProviderCleanupResponse::Cleaned {
            identity: request.identity,
            intent: request.intent,
        };
        reply(&self.client, &message, &cleaned).await
    }

    async fn control(&self, message: Message) -> Sendable<()> {
        let admitted = self
            .verifier
            .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)
            .ok()
            .flatten()
            .map(|verified| verified.scope().clone())
            .filter(|scope| binds_live_grant(&self.observed, scope));
        let Some(scope) = admitted else {
            locked(&self.observed).refused_controls += 1;
            return reply(
                &self.client,
                &message,
                &serde_json::json!({"error": "assignment_scope"}),
            )
            .await;
        };
        let operation = message
            .subject
            .as_str()
            .split('.')
            .nth(3)
            .unwrap_or_default()
            .to_owned();
        {
            let mut state = locked(&self.observed);
            if operation == "cancel" {
                state.live.remove(scope.lease_id.as_str());
            }
            state.controls.push(scope);
        }
        if operation != "run_command" {
            return Ok(());
        }
        let output = RunCommandResponse::Completed(CommandOutput {
            exit_code: 0,
            stdout: b"deliberately-wrong-output".to_vec(),
            stderr: Vec::new(),
            timed_out: CommandTimeoutStatus::WithinDeadline,
            truncated: OutputTruncation::Complete,
        });
        reply(&self.client, &message, &output).await
    }
}

async fn next(subscription: &mut async_nats::Subscriber) -> Sendable<Message> {
    subscription.next().await.ok_or_else(|| "a subscription ended".into())
}

impl AssignedWorker {
    async fn start(client: Client) -> Result<Self, Box<dyn Error>> {
        let signer = Arc::new(DispatchSigner::new("assigned-transport", [3; 32]));
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_key(signer.key_id(), &signer.public_key_b64())?
            .with_epoch_mode(EpochMode::Require);
        let worker_id = WorkerId::new("assigned-transport-worker")?;
        let mut dispatch = client.subscribe(worker_dispatch(KIND, &worker_id)).await?;
        let mut controls = client.subscribe("sandbox.provider.mock.*.*").await?;
        let mut cleanup = client.subscribe(worker_cleanup(KIND, &worker_id)).await?;
        client.flush().await?;
        let observed = Arc::new(Mutex::new(Observed::default()));
        let serving = Serving {
            client: client.clone(),
            worker_id: worker_id.clone(),
            verifier,
            observed: observed.clone(),
        };
        let task = tokio::spawn(async move {
            let mut beat = tokio::time::interval(Duration::from_millis(20));
            loop {
                tokio::select! {
                    _ = beat.tick() => serving.heartbeat().await?,
                    message = next(&mut dispatch) => serving.dispatch(message?).await?,
                    message = next(&mut cleanup) => serving.cleanup(message?).await?,
                    message = next(&mut controls) => serving.control(message?).await?,
                }
            }
        });
        Ok(Self {
            client,
            worker_id,
            signer,
            observed,
            task,
        })
    }

    async fn observe_heartbeat(&self) -> Result<WorkerHeartbeat, Box<dyn Error>> {
        let mut heartbeats = self.client.subscribe(worker_heartbeat_filter(KIND)).await?;
        self.client.flush().await?;
        let message = wire::local(wire::next(&mut heartbeats).await)?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        assert_eq!(heartbeat.dispatch_protocol, WorkerDispatchProtocol::TargetedV1);
        assert_eq!(heartbeat.supports_lease_epoch, CapabilitySupport::Supported);
        assert!(SystemTime::now().duration_since(heartbeat.emitted_at)? < wire::WAIT);
        Ok(heartbeat)
    }

    async fn target(&self, ledger: GrantLedger) -> Result<Target, Box<dyn Error>> {
        let heartbeat = self.observe_heartbeat().await?;
        let mut target = Target::new(self.client.clone(), KIND)
            .for_worker(&heartbeat)?
            .with_grant_ledger(ledger);
        target.signer = Some(self.signer.clone());
        target.timeout = wire::WAIT;
        Ok(target)
    }

    async fn assert_released(&self, expected: usize) -> Result<(), Box<dyn Error>> {
        tokio::time::timeout(wire::WAIT, async {
            loop {
                let released = {
                    let state = locked(&self.observed);
                    state.grants.len() == expected && state.live.is_empty()
                };
                if released {
                    return Ok(());
                }
                if self.task.is_finished() {
                    return Err("the assigned worker stopped serving");
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await??;
        Ok(())
    }

    async fn wait_for_refusal(&self) -> Result<(), Box<dyn Error>> {
        tokio::time::timeout(wire::WAIT, async {
            while locked(&self.observed).refused_controls == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        Ok(())
    }
}

async fn reply(client: &Client, message: &Message, value: &impl Serialize) -> Sendable<()> {
    if let Some(reply) = message.reply.clone() {
        client.publish(reply, serde_json::to_vec(value)?.into()).await?;
    }
    Ok(())
}

async fn signed_cancel(client: &Client, signer: &DispatchSigner, lease: &LeaseId, scope: &VerbScope) -> Sendable<()> {
    let subject = provider_cancel(KIND, lease);
    let mut headers = HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &[], scope)?.as_str());
    client.publish_with_headers(subject, headers, Bytes::new()).await?;
    client.flush().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_assigned_worker_grants_a_signed_dispatch_and_refuses_an_unscoped_cancel() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = AssignedWorker::start(client.clone()).await?;
    let heartbeat = worker.observe_heartbeat().await?;
    assert_eq!(heartbeat.worker_id, worker.worker_id);
    let lease = LeaseId::new("assigned-transport-lease");
    let assignment = DispatchTarget {
        worker_id: heartbeat.worker_id,
        attempt_id: DispatchAttemptId::new("assigned-transport-attempt")?,
        epoch: Some(LeaseEpoch::FIRST),
    };
    let tenant = TenantId::new(TENANT)?;
    let payload = ProviderDispatchPayload {
        lease_id: lease.clone(),
        request: support::request::lease_request(tenant.clone(), "assigned-transport-key")?,
        correlation_id: None,
        resolved_drives: Vec::new(),
        target: Some(assignment.clone()),
    };
    let signed = worker
        .signer
        .sign_payload(&serde_json::to_vec(&payload)?, OP_LEASE_DISPATCH)?;
    let grant = tokio::time::timeout(
        wire::WAIT,
        client.request(worker_dispatch(KIND, &assignment.worker_id), signed.into()),
    )
    .await??;
    let native: ProviderLease = serde_json::from_slice(&grant.payload)?;
    assert_eq!(native.worker_id.as_ref(), Some(&assignment.worker_id));
    assert_eq!(native.attempt_id.as_ref(), Some(&assignment.attempt_id));
    assert_eq!(native.epoch, assignment.epoch);

    let mut scope = VerbScope {
        tenant,
        lease_id: lease.clone(),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    };
    wire::local(signed_cancel(&client, &worker.signer, &lease, &scope).await)?;
    worker.wait_for_refusal().await?;
    assert_eq!(
        locked(&worker.observed).live.len(),
        1,
        "an unscoped cancel leaves the grant live"
    );

    scope.worker_id = Some(assignment.worker_id.clone());
    scope.attempt_id = Some(assignment.attempt_id.clone());
    scope.epoch = assignment.epoch;
    wire::local(signed_cancel(&client, &worker.signer, &lease, &scope).await)?;
    worker.assert_released(1).await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicitly_selected_assigned_worker_receives_a_signed_dispatch_and_cancel() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = AssignedWorker::start(client).await?;
    let ledger = GrantLedger::default();
    let target = worker.target(ledger.clone()).await?;
    let (_, check) = checks_for_target(&target)
        .into_iter()
        .find(|(name, _)| *name == "lease_is_granted")
        .ok_or("the grant check is in the suite")?;
    let result = check(&target).await;
    assert!(
        result.is_ok(),
        "the observed assigned worker grants the lease: {result:?}"
    );
    worker.assert_released(1).await?;
    let state = locked(&worker.observed);
    let recorded = ledger.snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].lease_id, state.grants[0].0);
    assert_eq!(recorded[0].native, state.grants[0].1);
    assert_eq!(state.controls.len(), 1);
    assert_eq!(state.refused_controls, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_assigned_check_keeps_its_native_grant_and_exact_cancel_scope() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = AssignedWorker::start(client).await?;
    let ledger = GrantLedger::default();
    let target = worker.target(ledger.clone()).await?;
    let (_, check) = checks_for_target(&target)
        .into_iter()
        .find(|(name, _)| *name == "run_returns_stdout_and_exit_zero")
        .ok_or("the command check is in the suite")?;
    let result = check(&target).await;
    let failure = result.as_ref().err().map(ToString::to_string).unwrap_or_default();
    assert!(
        failure.contains("hello on stdout") && failure.contains(&format!("{:?}", b"deliberately-wrong-output")),
        "the check reaches the assigned command before it fails: {result:?}"
    );
    worker.assert_released(1).await?;
    let state = locked(&worker.observed);
    let recorded = ledger.snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].lease_id, state.grants[0].0);
    assert_eq!(recorded[0].native, state.grants[0].1);
    assert_eq!(state.controls.len(), 2);
    assert_eq!(state.refused_controls, 0);
    assert_eq!(state.controls[0].worker_id, state.controls[1].worker_id);
    assert_eq!(state.controls[0].attempt_id, state.controls[1].attempt_id);
    assert_eq!(state.controls[0].epoch, state.controls[1].epoch);
    Ok(())
}
