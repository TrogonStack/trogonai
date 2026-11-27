use std::net::SocketAddr;

use trogon_sandbox_contract::NetworkPolicy;

use crate::check_failure::ensure;
use crate::constants::{TLS_PREFLIGHT, TLS_PROBE};
use crate::held::Held;
use crate::peer_certificate::PeerCertificate;
use crate::probe_token::probe_token;
use crate::tls_probe_result::TlsProbeResult;
use crate::{CheckFailureError, Target};

async fn probe(held: &Held<'_>, endpoint: SocketAddr) -> Result<TlsProbeResult, CheckFailureError> {
    let host = endpoint.ip().to_string();
    let port = endpoint.port().to_string();
    let output = probe_token(held, &["python3", "-c", TLS_PROBE, &host, &port]).await?;
    serde_json::from_str(&output).map_err(|_| CheckFailureError::unexpected("valid TLS probe diagnostics", &output))
}

async fn positive_control(
    control: &Held<'_>,
    endpoint: SocketAddr,
    phase: &str,
) -> Result<PeerCertificate, CheckFailureError> {
    match probe(control, endpoint).await? {
        TlsProbeResult::Authenticated { certificate_sha256 } => Ok(certificate_sha256),
        blocked @ TlsProbeResult::Blocked {} => Err(CheckFailureError::unexpected(
            format!("the unrestricted {phase} control to authenticate TLS to {endpoint}"),
            &blocked,
        )),
    }
}

/// Denial on a worker whose firewall accepts TCP without forwarding application traffic: a TCP connect proves
/// nothing there, so an authenticated TLS control before and after must reach the same peer while the restricted
/// lease sees its handshake ended.
pub(crate) async fn denied_delivery(target: &Target) -> Result<(), CheckFailureError> {
    let endpoint = target.egress_probe_address;
    let control = Held::acquire_request(target, target.lease_request_with_network(NetworkPolicy::AllowAll)?).await?;
    let result = restricted_delivery_fails(target, &control, endpoint).await;
    control.release().await;
    result
}

async fn restricted_delivery_fails(
    target: &Target,
    control: &Held<'_>,
    endpoint: SocketAddr,
) -> Result<(), CheckFailureError> {
    let marker = probe_token(control, &["python3", "-c", TLS_PREFLIGHT]).await?;
    ensure(
        marker == "PYTHON3_TLS",
        "the authenticated TLS preflight marker",
        &marker,
    )?;
    let before = positive_control(control, endpoint, "before").await?;
    let denied = Held::acquire_request(target, target.lease_request_with_network(NetworkPolicy::DenyAll)?).await?;
    let restricted = probe(&denied, endpoint).await;
    denied.release().await;
    let after = positive_control(control, endpoint, "after").await;
    let restricted = restricted?;
    let after = after?;
    ensure(
        after == before,
        "the unrestricted TLS peer to stay the same between controls",
        &after,
    )?;
    match restricted {
        TlsProbeResult::Blocked {} => Ok(()),
        TlsProbeResult::Authenticated { certificate_sha256 } if certificate_sha256 == before => {
            Err(CheckFailureError::unexpected(
                format!("a lease under DenyAll not to authenticate TLS to {endpoint}"),
                &certificate_sha256,
            ))
        }
        TlsProbeResult::Authenticated { certificate_sha256 } => Err(CheckFailureError::unexpected(
            "the restricted TLS peer to match the healthy control, so delivery is known",
            &certificate_sha256,
        )),
    }
}
