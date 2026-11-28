//! A bus that signs the broker's per-lease and cleanup requests.
//!
//! Every send goes through the wrapped bus. When the subject is a signed
//! request (see [`classify_subject`]) the lease is looked up in the active
//! lease store and the request carries an envelope binding the subject, the
//! body, the lease, its tenant and its dispatch attempt. A lease the store no
//! longer holds is signed with the tenant remembered from when it was last
//! seen, without a worker or attempt, so a late request such as a retried
//! cleanup still reaches a `require` worker. Anything else, including a lease
//! this broker never saw, is sent unsigned, which a worker in `warn` mode
//! accepts and one in `require` mode refuses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use tracing::warn;
use trogon_sandbox_contract::{
    CancelOwnedLeaseRequest, DispatchAttemptId, LeaseId, NativeSnapshotDeleteRequest, PurgeNamedRequest, TenantId,
};
use trogon_sandbox_protocol::{DispatchSigner, SignedSubject, VerbScope, classify_subject};
use trogon_std::env::ReadEnv;

use crate::active_lease::ActiveLeaseStore;
use crate::bus::BrokerBus;

mod constants;
mod signing_bus_error;

#[cfg(test)]
mod tests;

use constants::REMEMBERED_LEASES;
pub use signing_bus_error::SigningBusError;

pub struct SigningBus {
    inner: Arc<dyn BrokerBus>,
    leases: Arc<dyn ActiveLeaseStore>,
    signer: Arc<DispatchSigner>,
    tenants: Mutex<HashMap<LeaseId, TenantId>>,
}

/// Wraps `inner` when a signing key is configured in the environment;
/// otherwise returns it untouched.
pub fn wrap_from_env(
    env: &impl ReadEnv,
    inner: impl BrokerBus + 'static,
    leases: &Arc<dyn ActiveLeaseStore>,
) -> Result<Arc<dyn BrokerBus>, SigningBusError> {
    let signer = DispatchSigner::from_env(env)?;
    Ok(match signer {
        Some(signer) => Arc::new(SigningBus::new(Arc::new(inner), Arc::clone(leases), Arc::new(signer))),
        None => Arc::new(inner),
    })
}

impl SigningBus {
    pub fn new(inner: Arc<dyn BrokerBus>, leases: Arc<dyn ActiveLeaseStore>, signer: Arc<DispatchSigner>) -> Self {
        Self {
            inner,
            leases,
            signer,
            tenants: Mutex::new(HashMap::new()),
        }
    }

    fn remember(&self, lease: &LeaseId, tenant: &TenantId) {
        let mut tenants = self.tenants.lock().unwrap_or_else(|e| e.into_inner());
        if tenants.len() >= REMEMBERED_LEASES && !tenants.contains_key(lease) {
            tenants.clear();
        }
        tenants.insert(lease.clone(), tenant.clone());
    }

    fn remembered(&self, lease: &LeaseId) -> Option<TenantId> {
        self.tenants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(lease)
            .cloned()
    }

    /// The lease whose record the request is bound to, or the final answer
    /// (`Err`) when the subject needs no lookup or its body names no lease.
    fn lease_to_look_up(&self, kind: &SignedSubject, payload: &[u8]) -> Result<LeaseId, Option<VerbScope>> {
        match kind {
            SignedSubject::LeaseVerb { lease } => Ok(lease.clone()),
            SignedSubject::Cleanup => serde_json::from_slice::<serde_json::Value>(payload)
                .ok()
                .and_then(|body| Some(LeaseId::new(body.pointer("/identity/lease_id")?.as_str()?)))
                .ok_or(None),
            SignedSubject::SshSession { .. }
            | SignedSubject::SnapshotExport
            | SignedSubject::WorkerCommandReadiness { .. } => Err(None),
            SignedSubject::NativeSnapshotDelete { provider } => {
                Err(serde_json::from_slice::<NativeSnapshotDeleteRequest>(payload)
                    .ok()
                    .filter(|request| {
                        trogon_sandbox_protocol::subjects::provider_token(request.reference.provider) == *provider
                    })
                    .map(|request| tenant_scope(request.tenant, LeaseId::new(""))))
            }
            SignedSubject::PurgeNamed => Err(serde_json::from_slice::<PurgeNamedRequest>(payload)
                .ok()
                .map(|request| tenant_scope(request.tenant, LeaseId::new("")))),
        }
    }

    /// The scope to sign for, `None` when there is no lease to bind to, and an
    /// error when the lease store could not be read, since a request sent
    /// unsigned then would be indistinguishable from a forged one.
    async fn scope_for(&self, kind: &SignedSubject, payload: &[u8]) -> anyhow::Result<Option<VerbScope>> {
        let lease = match self.lease_to_look_up(kind, payload) {
            Ok(lease) => lease,
            Err(done) => return Ok(done),
        };
        let record = match self.leases.get(&lease).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                let Some(tenant) = self.remembered(&lease) else {
                    return Ok(None);
                };
                let attempt_id = match kind {
                    SignedSubject::Cleanup => {
                        serde_json::from_slice::<serde_json::Value>(payload)
                            .ok()
                            .and_then(|body| {
                                DispatchAttemptId::new(body.pointer("/identity/target/attempt_id")?.as_str()?).ok()
                            })
                    }
                    _ => None,
                };
                return Ok(Some(VerbScope {
                    tenant,
                    lease_id: lease,
                    worker_id: None,
                    attempt_id,
                    epoch: None,
                }));
            }
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "lease {lease} lookup failed, refusing to send the request unsigned: {error}"
                ));
            }
        };
        self.remember(&lease, &record.tenant);
        Ok(Some(VerbScope {
            tenant: record.tenant,
            lease_id: lease,
            worker_id: record.worker_id,
            attempt_id: record.dispatch_attempt.as_ref().map(|attempt| attempt.id.clone()),
            epoch: record.dispatch_attempt.as_ref().and_then(|attempt| attempt.epoch),
        }))
    }

    /// The envelope for a send to `subject`, `None` to send it unsigned because
    /// the subject is not signed or there is no lease to bind it to, and an
    /// error when it should be signed but cannot be.
    async fn header(&self, subject: &str, payload: &[u8]) -> anyhow::Result<Option<String>> {
        let Some(kind) = classify_subject(subject) else {
            return Ok(None);
        };
        if let SignedSubject::LeaseVerb { lease } = &kind
            && subject.split('.').nth(3) == Some("cancel_owned")
        {
            let Ok(request) = serde_json::from_slice::<CancelOwnedLeaseRequest>(payload) else {
                return Ok(None);
            };
            if request.lease_id != *lease {
                return Ok(None);
            }
            let scope = tenant_scope(request.tenant, lease.clone());
            return self.sign(subject, payload, &scope).map(Some);
        }
        let Some(scope) = self.scope_for(&kind, payload).await? else {
            warn!(subject, "no lease to bind the request to; sending it unsigned");
            return Ok(None);
        };
        self.sign(subject, payload, &scope).map(Some)
    }

    fn sign(&self, subject: &str, payload: &[u8], scope: &VerbScope) -> anyhow::Result<String> {
        self.signer.sign_verb(subject, payload, scope).map_err(|error| {
            anyhow::anyhow!("request to {subject} could not be signed, refusing to send it unsigned: {error}")
        })
    }
}

fn tenant_scope(tenant: TenantId, lease_id: LeaseId) -> VerbScope {
    VerbScope {
        tenant,
        lease_id,
        worker_id: None,
        attempt_id: None,
        epoch: None,
    }
}

#[async_trait]
impl BrokerBus for SigningBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        match self.header(subject, &payload).await? {
            Some(auth) => self.inner.request_with_auth(subject, &auth, payload, None).await,
            None => self.inner.request(subject, payload).await,
        }
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        match self.header(subject, &payload).await? {
            Some(auth) => self.inner.publish_with_auth(subject, &auth, payload).await,
            None => self.inner.publish(subject, payload).await,
        }
    }

    async fn publish_with_msg_id(&self, subject: &str, msg_id: &str, payload: Bytes) -> anyhow::Result<()> {
        self.inner.publish_with_msg_id(subject, msg_id, payload).await
    }

    async fn request_with_timeout(&self, subject: &str, payload: Bytes, timeout: Duration) -> anyhow::Result<Bytes> {
        match self.header(subject, &payload).await? {
            Some(auth) => {
                self.inner
                    .request_with_auth(subject, &auth, payload, Some(timeout))
                    .await
            }
            None => self.inner.request_with_timeout(subject, payload, timeout).await,
        }
    }

    async fn request_with_auth(
        &self,
        subject: &str,
        auth: &str,
        payload: Bytes,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Bytes> {
        self.inner.request_with_auth(subject, auth, payload, timeout).await
    }

    async fn publish_with_auth(&self, subject: &str, auth: &str, payload: Bytes) -> anyhow::Result<()> {
        self.inner.publish_with_auth(subject, auth, payload).await
    }

    fn max_payload(&self) -> Option<usize> {
        self.inner.max_payload()
    }
}
