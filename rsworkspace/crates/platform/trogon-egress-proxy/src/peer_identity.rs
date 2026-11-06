use std::fmt;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use rustls::pki_types::CertificateDer;
use tokio::time::Instant;
use trogon_audit::{WorkloadPeerEvidenceFacts, WorkloadPeerFactsError};
use trogon_std::{EpochClock, SystemClock};
use trogon_workload_identity::{MaterialError, MaterialLease};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, WorkloadPeerEvidence, WorkloadRole};
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;

use crate::agent_id::AgentId;
use crate::agent_id::AgentIdError;
use crate::peer_certificate::{PeerIdentityError, agent_id_from_certificate};
use crate::trust_domain::TrustDomain;

use crate::constants::MAX_SESSION_LIFETIME;

/// Identity and finite validity extracted from an already verified TLS peer.
/// The deadline also limits each session to 330 seconds; it cannot establish
/// that a static policy source learned about revocation before a reconnect.
#[derive(Clone)]
pub struct PeerIdentity {
    agent: AgentId,
    expires_at: SystemTime,
    deadline: Instant,
    workload: Option<WorkloadSession>,
}

#[derive(Clone)]
struct WorkloadSession {
    lease: MaterialLease,
    peer: WorkloadPeerEvidenceFacts,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkloadPeerIdentityError {
    #[error("workload peer cannot be verified")]
    Material(#[source] MaterialError),
    #[error("registered application identity is unsupported")]
    Application(#[source] AgentIdError),
    #[error("workload peer path has no usable lifetime")]
    Validity(#[source] PeerIdentityError),
    #[error("workload peer audit evidence is invalid")]
    Evidence(#[source] WorkloadPeerFactsError),
}

impl fmt::Debug for PeerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerIdentity")
            .field("agent", &self.agent)
            .field("expires_at", &self.expires_at)
            .field("deadline", &self.deadline)
            .field(
                "workload_peer",
                &self.workload.as_ref().map(|workload| workload.peer.message()),
            )
            .finish()
    }
}

impl PeerIdentity {
    pub fn from_workload_certificates(
        certificates: &[CertificateDer<'_>],
        lease: MaterialLease,
    ) -> Result<Self, WorkloadPeerIdentityError> {
        let peer = lease
            .peer_evidence_from_certificates(certificates)
            .map_err(WorkloadPeerIdentityError::Material)?;
        let agent = AgentId::new(peer.application_id().as_str()).map_err(WorkloadPeerIdentityError::Application)?;
        let binding = lease
            .registration()
            .bindings()
            .iter()
            .find(|binding| binding.spiffe_id() == peer.spiffe_id())
            .ok_or(WorkloadPeerIdentityError::Material(MaterialError::Identity))?;
        let fact = lease.fact();
        let evidence = WorkloadPeerEvidenceFacts::try_from(WorkloadPeerEvidence {
            peer_binding: MessageField::some(RegistrationBinding {
                spiffe_id: binding.spiffe_id().to_string(),
                tenant: binding.tenant().into(),
                project: binding.project().into(),
                role: EnumValue::from(WorkloadRole::Agent),
                application_id: Some(agent.as_str().into()),
            }),
            peer_leaf_sha256: peer.leaf_sha256().to_vec(),
            peer_path_expires_at: MessageField::some(Timestamp::from(peer.expires_at())),
            recovery_fence: fact.recovery_fence().get(),
            registration_revision: fact.registration_revision().get(),
            trust_revision: fact.trust_revision().get(),
            verifier_source_revision: fact.source_revision(),
            verifier_activation_revision: fact.activation_revision().get(),
            verifier_bundle_sha256: fact.bundle_sha256().to_vec(),
        })
        .map_err(WorkloadPeerIdentityError::Evidence)?;
        let mut identity = Self::with_clock(
            agent,
            peer.expires_at().min(lease.expires_at()),
            || SystemClock.system_time(),
        )
        .map_err(WorkloadPeerIdentityError::Validity)?;
        identity.deadline = identity.deadline.min(lease.deadline());
        if !lease.is_current() {
            return Err(WorkloadPeerIdentityError::Material(MaterialError::Unavailable));
        }
        identity.workload = Some(WorkloadSession { lease, peer: evidence });
        Ok(identity)
    }

    /// The caller must first authenticate this exact chain in the TLS handshake.
    /// Every presented certificate bounds validity conservatively. A trust anchor
    /// omitted from that chain needs a separate live trust-lifecycle boundary.
    pub fn from_verified_certificates(
        certificates: &[CertificateDer<'_>],
        trust_domain: &TrustDomain,
    ) -> Result<Self, PeerIdentityError> {
        let leaf = certificates.first().ok_or(PeerIdentityError::NoUriSan)?;
        let agent = agent_id_from_certificate(leaf.as_ref(), trust_domain)?;
        let mut expires_at = None;
        for der in certificates {
            let (rest, certificate) =
                X509Certificate::from_der(der.as_ref()).map_err(|_| PeerIdentityError::Unparsable)?;
            if !rest.is_empty() {
                return Err(PeerIdentityError::Unparsable);
            }
            let seconds =
                u64::try_from(certificate.validity().not_after.timestamp()).map_err(|_| PeerIdentityError::Validity)?;
            let expiry = SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(seconds))
                .ok_or(PeerIdentityError::Validity)?;
            expires_at = Some(expires_at.map_or(expiry, |current: SystemTime| current.min(expiry)));
        }
        Self::with_clock(
            agent,
            expires_at.ok_or(PeerIdentityError::Validity)?,
            || SystemClock.system_time(),
        )
    }

    fn with_clock(
        agent: AgentId,
        expires_at: SystemTime,
        read_wall: impl FnOnce() -> SystemTime,
    ) -> Result<Self, PeerIdentityError> {
        Self::at(agent, expires_at, read_wall())
    }

    fn at(agent: AgentId, expires_at: SystemTime, now: SystemTime) -> Result<Self, PeerIdentityError> {
        let remaining = expires_at
            .duration_since(now)
            .ok()
            .filter(|remaining| !remaining.is_zero())
            .ok_or(PeerIdentityError::Validity)?;
        Ok(Self {
            agent,
            expires_at,
            deadline: Instant::now() + remaining.min(MAX_SESSION_LIFETIME),
            workload: None,
        })
    }

    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn is_current(&self, now: SystemTime) -> bool {
        self.expires_at > now
            && self.deadline > Instant::now()
            && self
                .workload
                .as_ref()
                .is_none_or(|workload| workload.lease.is_current())
    }

    pub(crate) fn workload_peer(&self) -> Option<&WorkloadPeerEvidenceFacts> {
        self.workload.as_ref().map(|workload| &workload.peer)
    }

    pub(crate) async fn authority_changed(&self) {
        match &self.workload {
            Some(workload) => workload.lease.changed().await,
            None => std::future::pending().await,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(agent: AgentId, lifetime: Duration) -> Self {
        let now = SystemTime::now();
        Self::at(agent, now + lifetime, now).expect("test identity is current")
    }
}

#[cfg(test)]
mod tests;
