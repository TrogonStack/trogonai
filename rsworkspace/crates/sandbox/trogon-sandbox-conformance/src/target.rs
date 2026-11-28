use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::{Client, HeaderMap};
use bytes::Bytes;
use trogon_sandbox_contract::{
    CancelIntent, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, LeasePersistence, LeasePrewarm,
    LeaseRequest, NetworkPolicy, OciImageRef, ProviderCleanupRequest, ProviderCleanupResponse, ProviderKind,
    ProviderLease, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, worker_cleanup, worker_dispatch};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, SignedSubject, VerbScope, classify_subject, nats_transfer,
};

use crate::selected_worker::SelectedWorker;
use trogon_std::env::ReadEnv;

use crate::{
    AssignmentFaultError, CheckFailureError, ExecModel, GrantLedger, NativeCleanupAuthority, ids, signer_from_env,
};

#[cfg(test)]
mod tests;

/// A worker under test.
#[derive(Clone)]
pub struct Target {
    pub client: Client,
    pub kind: ProviderKind,
    /// What `run_command` means on this worker.
    pub exec: ExecModel,
    /// Image to lease. Ignored by workers that do not run images.
    pub image: String,
    /// Spec every check leases. `None` leases [`Target::image`] with an empty workspace; set it for workers that
    /// need more, such as a guest source.
    pub spec: Option<SandboxSpec>,
    /// Upper bound for any single request or wait.
    pub timeout: Duration,
    /// Require exactly exit 127 for an unknown command instead of any failure.
    pub strict_unknown_command_exit: bool,
    /// Runs detached commands that can be polled and killed; enables those checks. A worker that starts a fresh
    /// guest per command keeps none running, so it refuses them.
    pub detached_commands: bool,
    /// The worker declares `lifecycle.supports_pause_resume`; enables the pause and resume check.
    pub supports_pause_resume: bool,
    /// The worker declares `supports_graceful_stop`; enables the stop check.
    pub graceful_stop: bool,
    /// The worker declares `supports_command_restart`; enables the restart check.
    pub command_restart: bool,
    /// The worker declares `supports_named_processes`. Enables the named process checks; leave `false` for workers
    /// that reply unsupported.
    pub named_processes: bool,
    /// The worker declares `supports_process_readiness`. Enables the readiness checks.
    pub process_readiness: bool,
    /// A command that prints [`Target::process_needle`] and then runs until killed, used by the process checks.
    pub process_command: Vec<String>,
    /// What [`Target::process_command`] prints once it is up.
    pub process_needle: String,
    /// A command that listens on the paired TCP port until killed. When set, the readiness checks also cover a
    /// port probe.
    pub process_listener: Option<(Vec<String>, u16)>,
    /// The worker declares `snapshot.can_checkpoint_running == false`. Enables the check that a live-snapshot
    /// request gets a typed `Unsupported` reply instead of silence or a generic failure.
    pub checkpoint_running_unsupported: bool,
    /// The worker declares `placement.supports_native_passthrough`. Enables the check that an unknown `native` key
    /// is refused with a typed error.
    pub native_passthrough: bool,
    /// The worker keeps a lease's state when the same lease id is dispatched again (the broker redelivers a
    /// dispatch after a lost reply). Enables the re-dispatch check; leave `false` for workers that start a second
    /// sandbox.
    pub redispatch_idempotent: bool,
    /// The worker declares `isolation.supports_file_ops`. Enables the file verb checks whatever the [`ExecModel`],
    /// so a worker that serves files but no `run_command` is still covered.
    pub file_ops: bool,
    /// The worker declares `isolation.supports_file_search`.
    pub file_search: bool,
    /// The worker declares `isolation.supports_file_archive`.
    pub file_archive: bool,
    /// The worker offers `NetworkPolicy::DenyAll` and says its guest cannot reach the network under it. A worker
    /// that cannot set this should not report a stronger egress level than `BestEffort`.
    pub denied_connect_fails: bool,
    /// Literal IP destination used by the egress check, independent of DNS.
    pub egress_probe_address: SocketAddr,
    /// Directory the file verb checks create their scratch tree in. It must be reachable by the file verbs: `/tmp`
    /// on a worker that reaches the whole guest, `/workspace` on one that serves only the workspace.
    pub file_root: String,
    /// Signs every dispatch and per-lease request, for a worker that runs with `SANDBOX_DISPATCH_VERIFY=require`.
    /// `None` sends them unsigned, which is what a worker in `off` or `warn` mode expects.
    pub signer: Option<Arc<DispatchSigner>>,
    pub grants: Option<GrantLedger>,
    selected_worker: Option<SelectedWorker>,
    assignments: Arc<Mutex<HashMap<LeaseId, DispatchTarget>>>,
}

impl Target {
    pub fn new(client: Client, kind: ProviderKind) -> Self {
        Self {
            client,
            kind,
            exec: ExecModel::Shell,
            image: "alpine:3.20".to_string(),
            spec: None,
            timeout: Duration::from_secs(30),
            strict_unknown_command_exit: false,
            detached_commands: true,
            supports_pause_resume: false,
            graceful_stop: false,
            command_restart: false,
            named_processes: false,
            process_readiness: false,
            process_command: ["sh", "-c", "echo process-ready; sleep 300"].map(String::from).to_vec(),
            process_needle: "process-ready".to_string(),
            process_listener: None,
            checkpoint_running_unsupported: false,
            native_passthrough: false,
            redispatch_idempotent: false,
            file_ops: false,
            file_search: false,
            file_archive: false,
            denied_connect_fails: false,
            egress_probe_address: SocketAddr::from(([1, 1, 1, 1], 443)),
            file_root: "/tmp".to_string(),
            signer: None,
            grants: None,
            selected_worker: None,
            assignments: Arc::default(),
        }
    }

    pub fn with_grant_ledger(mut self, ledger: GrantLedger) -> Self {
        self.grants = Some(ledger);
        self
    }

    /// Directs every request at the worker that sent `heartbeat`, using the dispatch protocol it declares.
    pub fn for_worker(mut self, heartbeat: &WorkerHeartbeat) -> Result<Self, CheckFailureError> {
        if heartbeat.provider != self.kind || heartbeat.profile.provider != self.kind {
            return Err(AssignmentFaultError::ProviderMismatch.into());
        }
        self.selected_worker = Some(SelectedWorker {
            id: heartbeat.worker_id.clone(),
            protocol: heartbeat.dispatch_protocol,
            supports_lease_epoch: heartbeat.supports_lease_epoch,
        });
        self.assignments = Arc::default();
        Ok(self)
    }

    /// Signs requests with the key in `env`, if one is set.
    pub fn signed_from_env(mut self, env: &impl ReadEnv) -> Result<Self, CheckFailureError> {
        self.signer = signer_from_env(env)?;
        Ok(self)
    }

    fn assigned(&self) -> Option<&SelectedWorker> {
        self.selected_worker
            .as_ref()
            .filter(|worker| worker.protocol == WorkerDispatchProtocol::TargetedV1)
    }

    fn retained(&self, id: &LeaseId) -> Option<DispatchTarget> {
        self.assignments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    pub(crate) fn assignment(&self, id: &LeaseId) -> Result<Option<DispatchTarget>, CheckFailureError> {
        let Some(worker) = self.assigned() else {
            return Ok(None);
        };
        if let Some(assignment) = self.retained(id) {
            return Ok(Some(assignment));
        }
        let assignment = DispatchTarget {
            worker_id: worker.id.clone(),
            attempt_id: ids::fresh_attempt_id()?,
            epoch: worker.supports_lease_epoch.is_supported().then_some(LeaseEpoch::FIRST),
        };
        self.assignments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id.clone(), assignment.clone());
        Ok(Some(assignment))
    }

    pub(crate) fn dispatch_subject(&self, assignment: Option<&DispatchTarget>) -> String {
        assignment.map_or_else(
            || provider_dispatch(self.kind),
            |assignment| worker_dispatch(self.kind, &assignment.worker_id),
        )
    }

    pub(crate) fn validate_grant(
        &self,
        id: &LeaseId,
        lease: &ProviderLease,
    ) -> Result<NativeCleanupAuthority, AssignmentFaultError> {
        if lease.evidence_subject.is_empty() {
            return Err(AssignmentFaultError::GrantMissingEvidence);
        }
        if let Some(assignment) = self.retained(id) {
            let exact = lease.worker_id.as_ref() == Some(&assignment.worker_id)
                && lease.attempt_id.as_ref() == Some(&assignment.attempt_id)
                && lease.epoch == assignment.epoch;
            return exact
                .then_some(NativeCleanupAuthority::RequestMatched)
                .ok_or(AssignmentFaultError::GrantDiffersFromAssignment);
        }
        match &self.selected_worker {
            Some(worker) if worker.protocol == WorkerDispatchProtocol::Legacy => (lease.worker_id.as_ref()
                == Some(&worker.id))
            .then_some(NativeCleanupAuthority::RequestMatched)
            .ok_or(AssignmentFaultError::GrantDiffersFromSelectedWorker),
            Some(_) => Err(AssignmentFaultError::GrantWithoutAssignment),
            None => Ok(NativeCleanupAuthority::ReconciliationRequired),
        }
    }

    /// Records a grant in the ledger, if there is one, and returns the verdict on it.
    pub(crate) fn admit_grant(&self, id: &LeaseId, lease: &ProviderLease) -> Result<(), AssignmentFaultError> {
        let validation = self.validate_grant(id, lease);
        let authority = validation.unwrap_or(NativeCleanupAuthority::ReconciliationRequired);
        if let Some(ledger) = &self.grants {
            ledger.record(self.kind, id, lease, authority);
        }
        validation.map(|_| ())
    }

    /// The signed form of a request to `subject`: the body to send, and the auth header for a per-lease verb.
    /// Unsigned when no signer is set or the subject is not one a worker verifies.
    pub(crate) fn sign(
        &self,
        subject: &str,
        payload: Vec<u8>,
    ) -> Result<(Vec<u8>, Option<HeaderMap>), CheckFailureError> {
        let assigned = self.assigned();
        let Some(signer) = &self.signer else {
            return assigned.map_or(
                Ok((payload, None)),
                |_| Err(AssignmentFaultError::SignerRequired.into()),
            );
        };
        if subject == provider_dispatch(self.kind)
            || assigned.is_some_and(|worker| subject == worker_dispatch(self.kind, &worker.id))
        {
            return Ok((signer.sign_payload(&payload, OP_LEASE_DISPATCH)?, None));
        }
        let lease = match classify_subject(subject) {
            Some(SignedSubject::LeaseVerb { lease }) => lease,
            Some(SignedSubject::Cleanup) => self.cleanup_lease(subject, &payload)?,
            _ => return Ok((payload, None)),
        };
        let assignment = self.retained(&lease);
        if assigned.is_some() && assignment.is_none() {
            return Err(AssignmentFaultError::RequestWithoutAssignment.into());
        }
        let scope = VerbScope {
            tenant: ids::tenant()?,
            lease_id: lease,
            worker_id: assignment.as_ref().map(|target| target.worker_id.clone()),
            attempt_id: assignment.as_ref().map(|target| target.attempt_id.clone()),
            epoch: assignment.and_then(|target| target.epoch),
        };
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, signer.sign_verb(subject, &payload, &scope)?.as_str());
        Ok((payload, Some(headers)))
    }

    /// The lease a cleanup request is for, once it matches the retained assignment.
    fn cleanup_lease(&self, subject: &str, payload: &[u8]) -> Result<LeaseId, CheckFailureError> {
        let request: ProviderCleanupRequest =
            serde_json::from_slice(payload).map_err(|source| CheckFailureError::Decode {
                subject: subject.to_owned(),
                source,
                reply: crate::encoding::text(payload),
            })?;
        let worker = self.assigned().ok_or(AssignmentFaultError::CleanupWithoutWorker)?;
        let assignment = self
            .retained(&request.identity.lease_id)
            .ok_or(AssignmentFaultError::CleanupWithoutAssignment)?;
        let matches = request.identity.provider == self.kind
            && request.identity.target == assignment
            && assignment.worker_id == worker.id
            && subject == worker_cleanup(self.kind, &worker.id);
        matches
            .then_some(request.identity.lease_id)
            .ok_or_else(|| AssignmentFaultError::CleanupDiffers.into())
    }

    /// Sends `body` to `subject`, signed when configured, and decodes the reply.
    pub(crate) async fn call<T: serde::de::DeserializeOwned>(
        &self,
        subject: String,
        body: &impl serde::Serialize,
        timeout: Duration,
    ) -> Result<T, CheckFailureError> {
        let payload = serde_json::to_vec(body).map_err(CheckFailureError::Encode)?;
        let (payload, headers) = self.sign(&subject, payload)?;
        let reply = nats_transfer::request(&self.client, &subject, Bytes::from(payload), headers, timeout)
            .await
            .map_err(|source| CheckFailureError::Transport {
                subject: subject.clone(),
                source,
            })?;
        serde_json::from_slice(&reply).map_err(|source| CheckFailureError::Decode {
            subject,
            source,
            reply: crate::encoding::text(&reply),
        })
    }

    /// Releases the lease through the selected worker's protocol, signed when configured.
    pub(crate) async fn cancel(&self, id: &LeaseId) {
        if self.assigned().is_some() {
            if let Err(error) = self.release_assigned(id).await {
                tracing::warn!(%error, "assigned conformance cleanup remains unconfirmed");
            }
            return;
        }
        let subject = provider_cancel(self.kind, id);
        let (payload, headers) = self.sign(&subject, Vec::new()).unwrap_or_default();
        let _ = match headers {
            Some(headers) => self.client.publish_with_headers(subject, headers, payload.into()).await,
            None => self.client.publish(subject, payload.into()).await,
        };
        let _ = self.client.flush().await;
    }

    async fn release_assigned(&self, id: &LeaseId) -> Result<(), CheckFailureError> {
        let target = self
            .retained(id)
            .ok_or(AssignmentFaultError::CleanupWithoutAssignment)?;
        let request = ProviderCleanupRequest {
            identity: LeaseAttemptIdentity {
                lease_id: id.clone(),
                provider: self.kind,
                target: target.clone(),
            },
            intent: CancelIntent::Release,
        };
        let response: ProviderCleanupResponse = self
            .call(worker_cleanup(self.kind, &target.worker_id), &request, self.timeout)
            .await?;
        let confirmed = matches!(
            &response,
            ProviderCleanupResponse::Cleaned { identity, intent: CancelIntent::Release } if *identity == request.identity
        );
        confirmed
            .then_some(())
            .ok_or_else(|| AssignmentFaultError::CleanupNotConfirmed.into())
    }

    /// The lease request every check starts from.
    pub(crate) fn lease_request(&self) -> Result<LeaseRequest, CheckFailureError> {
        Ok(LeaseRequest {
            operation_id: None,
            tenant: ids::tenant()?,
            provider_pref: vec![self.kind],
            spec: self.spec()?,
            ttl_seconds: 120,
            idempotency_key: ids::fresh_request_key()?,
            isolation_tier: None,
            host_imports: vec![],
            required_capabilities: None,
            prewarm: LeasePrewarm::default(),
            tags: SandboxTags::default(),
            name: None,
            persistent: LeasePersistence::Persistent,
            native: None,
            region: None,
            fallback_regions: Vec::new(),
        })
    }

    /// The lease request with the given network policy on its spec.
    pub(crate) fn lease_request_with_network(&self, policy: NetworkPolicy) -> Result<LeaseRequest, CheckFailureError> {
        let mut request = self.lease_request()?;
        let mut parts = request.spec.into_parts();
        parts.network_policy = Some(policy);
        request.spec = SandboxSpec::new(parts)?;
        Ok(request)
    }

    fn spec(&self) -> Result<SandboxSpec, CheckFailureError> {
        self.spec.clone().map_or_else(
            || {
                Ok(SandboxSpec::new(SandboxSpecParts {
                    image: SandboxImage::Registry {
                        reference: OciImageRef::parse_unqualified(&self.image),
                        pull_secret: None,
                    },
                    workspace: WorkspaceSource::empty(),
                    env: Default::default(),
                    cpu_seconds: std::num::NonZeroU32::MIN.saturating_add(3599),
                    memory_mib: std::num::NonZeroU32::MIN.saturating_add(511),
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
            },
            Ok,
        )
    }
}
