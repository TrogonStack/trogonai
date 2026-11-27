use std::net::IpAddr;

use crate::connect_target::ConnectTarget;
use crate::ingress_rejection_error::IngressRejectionError;
use crate::proxy_outcome::ProxyOutcome;

/// Result of serving one explicit proxy connection. `dest_ip` is absent when
/// the name was never resolved.
#[derive(Debug)]
pub enum IngressOutcome {
    Proxied {
        target: ConnectTarget,
        dest_ip: Option<IpAddr>,
        outcome: ProxyOutcome,
    },
    Rejected(IngressRejectionError),
    /// The client fetched the revocation list.
    CrlServed,
}
