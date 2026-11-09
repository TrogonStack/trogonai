use std::time::{Duration, SystemTime};

use rustls::pki_types::CertificateDer;
use tokio::time::Instant;
use trogon_std::{EpochClock, SystemClock};
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;

use crate::agent_id::AgentId;
use crate::peer_certificate::{PeerIdentityError, agent_id_from_certificate};
use crate::trust_domain::TrustDomain;

use crate::constants::MAX_SESSION_LIFETIME;

/// Identity and finite validity extracted from an already verified TLS peer.
/// The deadline also limits each session to 330 seconds; it cannot establish
/// that a static policy source learned about revocation before a reconnect.
#[derive(Debug, Clone)]
pub struct PeerIdentity {
    agent: AgentId,
    expires_at: SystemTime,
    deadline: Instant,
}

impl PeerIdentity {
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
        Self::at(
            agent,
            expires_at.ok_or(PeerIdentityError::Validity)?,
            SystemClock.system_time(),
        )
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
        self.expires_at > now && self.deadline > Instant::now()
    }

    #[cfg(test)]
    pub(crate) fn for_test(agent: AgentId, lifetime: Duration) -> Self {
        let now = SystemTime::now();
        Self::at(agent, now + lifetime, now).expect("test identity is current")
    }
}

#[cfg(test)]
mod tests;
