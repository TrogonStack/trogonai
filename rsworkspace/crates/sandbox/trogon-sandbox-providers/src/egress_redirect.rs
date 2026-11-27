mod constants;

use std::fmt::Display;

use trogon_sandbox_contract::ProxyUid;

use crate::ProxyUidWorkloadError;

pub use constants::{
    EGRESS_PROXY_EXEMPT_UID_ENV, EGRESS_PROXY_LEASE_ID_ENV, EGRESS_PROXY_VIOLATION_NATS_URL_ENV,
    UID_CHANGE_CAPABILITIES,
};

/// Replaces a loopback host (`127.0.0.1`, `localhost`, `[::1]`) in a URL with
/// `replacement`; any other URL is returned unchanged.
pub fn rewrite_loopback_host(url: &str, replacement: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, authority),
    };
    let (host, port) = if let Some(bracketed) = hostport.strip_prefix('[') {
        match bracketed.split_once(']') {
            Some((host, port)) => (format!("[{host}]"), port),
            None => return url.to_string(),
        }
    } else {
        match hostport.split_once(':') {
            Some((host, _)) => (host.to_string(), &hostport[host.len()..]),
            None => (hostport.to_string(), ""),
        }
    };
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
        return url.to_string();
    }
    let userinfo = userinfo.map(|userinfo| format!("{userinfo}@")).unwrap_or_default();
    format!("{scheme}://{userinfo}{replacement}{port}{tail}")
}

/// Shell command installing the transparent redirect to the proxy.
///
/// A `RETURN` rule for TCP owned by [`ProxyUid::EGRESS_PROXY`] precedes the
/// redirect, so everything the proxy dials, NATS included, is never redirected
/// back into its own listener. No other uid is exempted, which holds only
/// while the workload cannot become that uid: it must not run as it and must
/// not keep CAP_SETUID or CAP_SETGID.
pub fn egress_redirect_command(listen_port: impl Display) -> String {
    [
        format!(
            "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner {} -j RETURN",
            ProxyUid::EGRESS_PROXY
        ),
        format!("iptables -t nat -A OUTPUT -p tcp ! -d 127.0.0.1 -j REDIRECT --to-port {listen_port}"),
    ]
    .join(" && ")
}

/// The environment pair that tells the proxy which uid the redirect exempts.
pub fn exempt_uid_env_pair() -> (&'static str, String) {
    (EGRESS_PROXY_EXEMPT_UID_ENV, ProxyUid::EGRESS_PROXY.to_string())
}

/// Rejects a workload `user` (`"uid"`, `"uid:gid"` or a name) that is the
/// proxy uid, since the uid exemption would then cover the workload.
pub fn reject_proxy_uid_workload(user: &str) -> Result<(), ProxyUidWorkloadError> {
    let uid = user.split(':').next().unwrap_or(user);
    match uid.parse::<ProxyUid>() {
        Ok(uid) if uid == ProxyUid::EGRESS_PROXY => Err(ProxyUidWorkloadError {
            user: user.to_string(),
            uid,
        }),
        _ => Ok(()),
    }
}
