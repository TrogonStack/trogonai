use serde::Deserialize;

use crate::peer_certificate::PeerCertificate;

/// What the TLS probe printed: an authenticated handshake with its peer, or a handshake the network ended.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TlsProbeResult {
    Authenticated { certificate_sha256: PeerCertificate },
    Blocked {},
}

#[cfg(test)]
mod tests;
