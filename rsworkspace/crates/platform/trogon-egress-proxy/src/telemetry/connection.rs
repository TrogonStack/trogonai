//! One event per accepted mTLS connection, after the peer's identity is
//! extracted and before its first request is read. It names the verified peer
//! and where the credentials came from, never certificate or key material.

use std::net::SocketAddr;

use crate::credential_source::CredentialSource;

pub(crate) fn accepted(peer: SocketAddr, spiffe_id: &str, source: CredentialSource) {
    tracing::info!(
        event = "peer_connection_accepted",
        %peer,
        peer.spiffe_id = spiffe_id,
        credential_source = source.as_str(),
        "mTLS connection accepted"
    );
}

#[cfg(test)]
mod tests;
