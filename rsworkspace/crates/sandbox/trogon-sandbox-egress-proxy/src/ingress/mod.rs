//! Explicit proxy ingress: HTTP CONNECT, absolute-form HTTP and SOCKS5 on one
//! listener, behind a per-lease token. The destination named by the client
//! (host and port) is the policy input, so unlike transparent mode no SNI has
//! to be sniffed for a decision.
//!
//! The request flow is: dispatch on the first byte ([`mux`]), authenticate
//! ([`token`]), parse the destination ([`connect`], [`socks5`]), pre-screen it
//! by name ([`crate::policy::decide_host`]), resolve and vet ([`crate::resolve::VettingResolver`]),
//! decide again against the address, answer the client, then hand the
//! connection to [`crate::proxy`]'s forwarding.

use std::time::Duration;

pub use crate::client_io::ClientIo;
pub use crate::connect_target::ConnectTarget;
pub use crate::ingress_outcome::IngressOutcome;
pub use crate::ingress_rejection_error::IngressRejectionError;

mod admission;
pub mod connect;
mod denied;
mod denied_request;
pub mod deny_header;
mod ingress_core;
pub mod listener;
pub mod mux;
mod plain;
mod refused;
pub mod socks5;
pub mod token;
mod unreachable;

pub use ingress_core::Ingress;
pub use listener::{AcceptedStream, ListenAddress, Listener, PeerAddr};
pub use mux::Protocol;
pub use token::{InvalidProxyTokenError, ProxyToken};

mod constants;
pub use crate::constants::KEEP_ALIVE_TIMEOUT;
pub use constants::{
    HANDSHAKE_TIMEOUT, HEAD_TIMEOUT, KEEP_ALIVE_GRACE, MAX_HEAD_BYTES, PROXY_ONLY_MAX_HEAD_BYTES, RESOLVE_TIMEOUT,
};

/// Bounds applied to every explicit proxy connection.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct IngressLimits {
    pub handshake_timeout: Duration,
    pub head_timeout: Duration,
    pub keep_alive_timeout: Duration,
    pub resolve_timeout: Duration,
    pub max_head_bytes: usize,
    pub ascii_hosts_only: bool,
}

impl IngressLimits {
    /// The limits of a proxy that serves clients outside a sandbox, which
    /// accepts the larger request heads such clients send and takes only
    /// destination hosts spelled in plain ASCII.
    pub fn proxy_only() -> Self {
        Self {
            max_head_bytes: PROXY_ONLY_MAX_HEAD_BYTES,
            ascii_hosts_only: true,
            ..Self::default()
        }
    }
}

impl Default for IngressLimits {
    fn default() -> Self {
        Self {
            handshake_timeout: HANDSHAKE_TIMEOUT,
            head_timeout: HEAD_TIMEOUT,
            keep_alive_timeout: KEEP_ALIVE_TIMEOUT,
            resolve_timeout: RESOLVE_TIMEOUT,
            max_head_bytes: MAX_HEAD_BYTES,
            ascii_hosts_only: false,
        }
    }
}
