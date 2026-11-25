use std::time::SystemTime;

use tokio::time::Instant;

use crate::PeerEvidence;

/// How the token reached the verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopKind {
    MutualTls,
    UnixSocket,
}

impl HopKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MutualTls => "mutual_tls",
            Self::UnixSocket => "unix_socket",
        }
    }
}

/// Proof that a JWT-SVID arrived over a confidential hop. A JWT-SVID is a
/// bearer token, so [`crate::JwtSvidVerifier::verify`] takes one of these with
/// every token. The only constructors are an authenticated mutual-TLS peer and a
/// local Unix domain socket peer; there is no way to assert confidentiality
/// without one of them.
#[derive(Debug, Clone)]
pub struct ConfidentialHop(Hop);

#[derive(Debug, Clone)]
enum Hop {
    MutualTls(PeerEvidence),
    #[cfg(unix)]
    UnixSocket {
        uid: u32,
        pid: Option<i32>,
    },
}

impl ConfidentialHop {
    /// The token arrived over a TLS connection whose client certificate chain
    /// was authenticated by [`crate::MaterialLease::peer_evidence_from_certificates`].
    /// The hop is refused once that evidence expires.
    pub fn mutual_tls(peer: &PeerEvidence) -> Self {
        Self(Hop::MutualTls(peer.clone()))
    }

    /// The token arrived over a connected local Unix domain socket, the one
    /// accepted exception to the confidential-hop rule. The peer credentials are
    /// read from the kernel, so a stream that cannot report them is refused.
    #[cfg(unix)]
    pub fn unix_socket(stream: &tokio::net::UnixStream) -> std::io::Result<Self> {
        let credentials = stream.peer_cred()?;
        Ok(Self(Hop::UnixSocket {
            uid: credentials.uid(),
            pid: credentials.pid(),
        }))
    }

    pub fn kind(&self) -> HopKind {
        match &self.0 {
            Hop::MutualTls(_) => HopKind::MutualTls,
            #[cfg(unix)]
            Hop::UnixSocket { .. } => HopKind::UnixSocket,
        }
    }

    /// The authenticated mutual-TLS peer, when that is the hop kind.
    pub fn peer_evidence(&self) -> Option<&PeerEvidence> {
        match &self.0 {
            Hop::MutualTls(peer) => Some(peer),
            #[cfg(unix)]
            Hop::UnixSocket { .. } => None,
        }
    }

    /// The kernel-reported `(uid, pid)` of a Unix socket peer.
    #[cfg(unix)]
    pub fn unix_peer(&self) -> Option<(u32, Option<i32>)> {
        match &self.0 {
            Hop::UnixSocket { uid, pid } => Some((*uid, *pid)),
            Hop::MutualTls(_) => None,
        }
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.peer_evidence().map(PeerEvidence::deadline)
    }

    pub(crate) fn expires_at(&self) -> Option<SystemTime> {
        self.peer_evidence().map(PeerEvidence::expires_at)
    }
}
