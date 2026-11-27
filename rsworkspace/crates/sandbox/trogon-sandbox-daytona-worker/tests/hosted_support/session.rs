//! Leases and per-lease verbs against the hosted worker, built on the public wire types.
//!
//! The conformance target keeps its lease handling to itself, so a lane that needs one lease across many verbs, a
//! native snapshot restore, or a terminal signs and sends its own requests here. Every lease a session acquires is
//! remembered so the lane can prove the sandboxes are gone.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_nats::{Client, HeaderMap};
use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, LeasePersistence,
    LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    TenantId, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, worker_cleanup, worker_dispatch};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, SignedSubject, VerbScope, classify_subject, nats_transfer,
};

use crate::hosted_support::Boxed;

const TENANT: &str = "daytona-hosted";

/// A lease the session holds, with the grant the worker returned.
pub struct Held {
    pub id: LeaseId,
    pub grant: ProviderLease,
}

/// How a session reaches one worker.
pub struct Session {
    client: Client,
    signer: Option<std::sync::Arc<DispatchSigner>>,
    protocol: WorkerDispatchProtocol,
    worker: WorkerId,
    epochs: bool,
    image: String,
    pub timeout: Duration,
    assignments: Mutex<HashMap<LeaseId, DispatchTarget>>,
    sandboxes: Mutex<Vec<String>>,
    leases: Mutex<Vec<LeaseId>>,
}

impl Session {
    pub fn new(
        client: Client,
        heartbeat: &WorkerHeartbeat,
        signer: Option<std::sync::Arc<DispatchSigner>>,
        image: &str,
    ) -> Self {
        Self {
            client,
            signer,
            protocol: heartbeat.dispatch_protocol,
            worker: heartbeat.worker_id.clone(),
            epochs: heartbeat.supports_lease_epoch.is_supported(),
            image: image.to_owned(),
            timeout: Duration::from_secs(120),
            assignments: Mutex::new(HashMap::new()),
            sandboxes: Mutex::new(Vec::new()),
            leases: Mutex::new(Vec::new()),
        }
    }

    /// Asks the worker to release every lease the session acquired, counting the releases it could not confirm.
    pub async fn release_all(&self) -> usize {
        let leases = self
            .leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut failed = 0;
        for lease in &leases {
            if self.cancel(lease).await.is_err() {
                failed += 1;
            }
        }
        failed
    }

    /// The native id of every sandbox this session was granted.
    pub fn sandbox_ids(&self) -> Vec<String> {
        self.sandboxes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn targeted(&self) -> bool {
        self.protocol == WorkerDispatchProtocol::TargetedV1
    }

    fn assignment(&self, id: &LeaseId) -> Option<DispatchTarget> {
        self.assignments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
    }

    /// The configured snapshot as a lease image.
    pub fn configured_image(&self) -> SandboxImage {
        SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(&self.image),
            pull_secret: None,
        }
    }

    /// Leases a sandbox from `image` and records its native id.
    pub async fn acquire(&self, image: SandboxImage) -> Result<Held, Boxed> {
        let id = LeaseId::new(uuid::Uuid::now_v7().to_string());
        let assignment = if self.targeted() {
            let target = DispatchTarget {
                worker_id: self.worker.clone(),
                attempt_id: DispatchAttemptId::new(uuid::Uuid::now_v7().to_string())?,
                epoch: self.epochs.then_some(LeaseEpoch::FIRST),
            };
            self.assignments
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id.clone(), target.clone());
            Some(target)
        } else {
            None
        };
        let request = lease_request(image)?;
        let payload = ProviderDispatchPayload {
            lease_id: id.clone(),
            request,
            correlation_id: None,
            resolved_drives: vec![],
            target: assignment.clone(),
        };
        let subject = assignment.as_ref().map_or_else(
            || provider_dispatch(ProviderKind::Daytona),
            |target| worker_dispatch(ProviderKind::Daytona, &target.worker_id),
        );
        let grant: ProviderLease = self.call(subject, &payload, self.timeout).await?;
        self.sandboxes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(grant.sandbox_id.as_str().to_owned());
        self.leases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(id.clone());
        Ok(Held { id, grant })
    }

    /// Signs `payload` for `subject`: a dispatch body, or an auth header for a per-lease verb.
    fn sign(
        &self,
        subject: &str,
        payload: Vec<u8>,
        lease: Option<&LeaseId>,
    ) -> Result<(Vec<u8>, Option<HeaderMap>), Boxed> {
        let Some(signer) = &self.signer else {
            return if self.targeted() {
                Err("a worker that takes targeted dispatch needs the signing key".into())
            } else {
                Ok((payload, None))
            };
        };
        let Some(lease) = lease else {
            return Ok((signer.sign_payload(&payload, OP_LEASE_DISPATCH)?, None));
        };
        let assignment = self.assignment(lease);
        let scope = VerbScope {
            tenant: TenantId::new(TENANT)?,
            lease_id: lease.clone(),
            worker_id: assignment.as_ref().map(|target| target.worker_id.clone()),
            attempt_id: assignment.as_ref().map(|target| target.attempt_id.clone()),
            epoch: assignment.and_then(|target| target.epoch),
        };
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, signer.sign_verb(subject, &payload, &scope)?.as_str());
        Ok((payload, Some(headers)))
    }

    /// Sends `body` to a lease's dispatch or verb `subject` and decodes the reply.
    pub async fn call<T: DeserializeOwned>(
        &self,
        subject: String,
        body: &impl Serialize,
        timeout: Duration,
    ) -> Result<T, Boxed> {
        let lease = self.lease_of(&subject, body)?;
        let (payload, headers) = self.sign(&subject, serde_json::to_vec(body)?, lease.as_ref())?;
        let reply = nats_transfer::request(&self.client, &subject, Bytes::from(payload), headers, timeout).await?;
        Ok(serde_json::from_slice(&reply)?)
    }

    /// The lease a request is for: none for a dispatch, the lease the subject or a cleanup body names otherwise.
    fn lease_of(&self, subject: &str, body: &impl Serialize) -> Result<Option<LeaseId>, Boxed> {
        if subject == provider_dispatch(ProviderKind::Daytona)
            || subject == worker_dispatch(ProviderKind::Daytona, &self.worker)
        {
            return Ok(None);
        }
        match classify_subject(subject) {
            Some(SignedSubject::LeaseVerb { lease }) => Ok(Some(lease)),
            Some(SignedSubject::Cleanup) => {
                let value = serde_json::to_value(body)?;
                let lease = value
                    .get("identity")
                    .and_then(|identity| identity.get("lease_id"))
                    .and_then(|lease| lease.as_str())
                    .ok_or("a cleanup request names no lease")?;
                Ok(Some(LeaseId::new(lease)))
            }
            _ => Err(format!("{subject} is not a subject this session signs").into()),
        }
    }

    /// Releases the lease through the worker's protocol.
    pub async fn cancel(&self, id: &LeaseId) -> Result<(), Boxed> {
        if let Some(target) = self.assignment(id) {
            let request = ProviderCleanupRequest {
                identity: LeaseAttemptIdentity {
                    lease_id: id.clone(),
                    provider: ProviderKind::Daytona,
                    target,
                },
                intent: CancelIntent::Release,
            };
            let subject = worker_cleanup(ProviderKind::Daytona, &self.worker);
            let response: ProviderCleanupResponse = self.call(subject, &request, self.timeout).await?;
            return match response {
                ProviderCleanupResponse::Cleaned { identity, .. } if identity == request.identity => Ok(()),
                _ => Err("the worker did not confirm the release".into()),
            };
        }
        let subject = provider_cancel(ProviderKind::Daytona, id);
        let (payload, headers) = self.sign(&subject, Vec::new(), Some(id))?;
        match headers {
            Some(headers) => {
                self.client
                    .publish_with_headers(subject, headers, payload.into())
                    .await?
            }
            None => self.client.publish(subject, payload.into()).await?,
        }
        self.client.flush().await?;
        Ok(())
    }
}

/// A persistent one-hour lease request for `image`, with the defaults for everything else.
pub fn lease_request(image: SandboxImage) -> Result<LeaseRequest, Boxed> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image,
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
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new(TENANT)?,
        provider_pref: vec![ProviderKind::Daytona],
        spec,
        ttl_seconds: 900,
        idempotency_key: LeaseRequestKey::new(uuid::Uuid::now_v7().to_string())?,
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
