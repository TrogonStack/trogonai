//! The policy switch that refuses a tunnel whose bytes the proxy cannot
//! read: one it would splice unread, or one it would terminate whose first
//! bytes are not a TLS handshake.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::timeout;
use trogon_sandbox_contract::{DomainRuleAction, EgressViolationReason, OpaqueTunnelPolicy};

use crate::denial::Denial;

mod constants;
use constants::{RECORD_HEADER_JUDGED, TLS_HANDSHAKE_RECORD};

/// What the first bytes of a tunnel to be terminated turned out to be.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum FirstBytes {
    /// A TLS handshake record, or nothing yet (the handshake deadline
    /// answers a client that stays silent). Carries every byte read.
    Terminable(Vec<u8>),
    NotTls,
}

pub(crate) fn refusal(host: Option<&str>) -> Denial {
    Denial::new(host.map(str::to_string), EgressViolationReason::OpaqueTunnelRefused)
}

/// Whether a connection allowed under `action` would be carried unread:
/// no domain rule asks for termination and no secret binding covers the
/// host (which terminates whatever its rule says).
pub(crate) fn would_splice_unread(action: Option<&DomainRuleAction>, terminated_by_secrets: bool) -> bool {
    !terminated_by_secrets && matches!(action, None | Some(DomainRuleAction::PassThrough))
}

pub(crate) async fn first_bytes<S>(
    client: &mut S,
    prefix: Vec<u8>,
    wait: Duration,
    policy: OpaqueTunnelPolicy,
) -> FirstBytes
where
    S: AsyncRead + Unpin,
{
    if !policy.refuses() {
        return FirstBytes::Terminable(prefix);
    }
    let prefix = read_first_bytes(client, prefix, wait).await;
    if judged_not_tls(&prefix) {
        FirstBytes::NotTls
    } else {
        FirstBytes::Terminable(prefix)
    }
}

/// Reads until the client has sent a whole record header, the proxy does, giving up
/// when the client ends or after `wait`, so a silent client is left to the
/// handshake deadline.
pub(crate) async fn read_first_bytes<S>(client: &mut S, mut prefix: Vec<u8>, wait: Duration) -> Vec<u8>
where
    S: AsyncRead + Unpin,
{
    let read = async {
        let mut buf = [0u8; 4096];
        while prefix.len() < RECORD_HEADER_JUDGED {
            match client.read(&mut buf).await {
                Ok(n) if n > 0 => prefix.extend_from_slice(&buf[..n]),
                _ => break,
            }
        }
    };
    let _ = timeout(wait, read).await;
    prefix
}

/// Whether the prefix looks like a client hello: a
/// handshake record whose legacy version is 3.0 to 3.3, judged by the bytes
/// there are.
pub(crate) fn starts_like_tls(prefix: &[u8]) -> bool {
    const LEGACY_RECORD_MAJOR: u8 = 0x03;
    const LEGACY_RECORD_MINOR_MAX: u8 = 0x03;
    let [first, second, third] = [0, 1, 2].map(|at| prefix.get(at));
    first.is_none_or(|&byte| byte == TLS_HANDSHAKE_RECORD)
        && second.is_none_or(|&byte| byte == LEGACY_RECORD_MAJOR)
        && third.is_none_or(|&byte| byte <= LEGACY_RECORD_MINOR_MAX)
}

/// The proxy reads a whole record header before it judges. A client that ends or stays silent short of one is not judged:
/// the decision window destroys its connection, and
/// here the handshake does, with nothing relayed either way.
pub(crate) fn judged_not_tls(prefix: &[u8]) -> bool {
    prefix.len() >= RECORD_HEADER_JUDGED && !starts_like_tls(prefix)
}

#[cfg(test)]
mod tests;
