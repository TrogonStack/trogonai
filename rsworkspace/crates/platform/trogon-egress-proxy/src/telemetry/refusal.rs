//! Why a connection was dropped before any HTTP byte. `reason` is a stable
//! code. The peer's SPIFFE ID is logged only for a certificate the handshake
//! verified, and never certificate, key or token material.

use std::net::SocketAddr;

use rustls::{CertificateError, Error};

use crate::svid_lifetime::SvidLifetimeError;

pub(crate) fn handshake_refused(peer: SocketAddr, error: &std::io::Error) {
    if exceeds_svid_lifetime(error) {
        tracing::warn!(
            %peer,
            reason = "client_certificate_lifetime_exceeded",
            %error,
            "client certificate outlives the X.509-SVID ceiling"
        );
        return;
    }
    tracing::warn!(%peer, reason = "tls_handshake_refused", %error, "TLS handshake refused");
}

fn exceeds_svid_lifetime(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<Error>())
        .is_some_and(|error| {
            matches!(
                error,
                Error::InvalidCertificate(CertificateError::Other(other))
                    if other.0.downcast_ref::<SvidLifetimeError>().is_some()
            )
        })
}

pub(crate) fn handshake_timed_out(peer: SocketAddr) {
    tracing::warn!(%peer, reason = "tls_handshake_timed_out", "TLS handshake timed out");
}

pub(crate) fn workload_peer_refused(peer: SocketAddr, spiffe_id: Option<&str>, error: &dyn std::error::Error) {
    tracing::warn!(
        %peer,
        reason = "workload_peer_refused",
        peer.spiffe_id = spiffe_id,
        %error,
        "workload peer refused"
    );
}

pub(crate) fn certificate_names_no_agent(peer: SocketAddr, spiffe_id: Option<&str>, error: &dyn std::error::Error) {
    tracing::warn!(
        %peer,
        reason = "client_certificate_names_no_agent",
        peer.spiffe_id = spiffe_id,
        %error,
        "client certificate names no agent"
    );
}
