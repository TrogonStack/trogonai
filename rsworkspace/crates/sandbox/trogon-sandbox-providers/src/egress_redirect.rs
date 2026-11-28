mod constants;

use std::fmt::Display;

use trogon_sandbox_contract::ProxyUid;

use crate::{EgressLockdownRules, ProxyUidWorkloadError};

use constants::{DNS_PORT, IPV6_STACK_PROBE};
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
///
/// This steers TCP only. A namespace whose only egress control is this redirect still lets UDP, ICMP and IPv6 out;
/// use [`egress_lockdown_command`] where nothing else (such as a Kubernetes `NetworkPolicy`) bounds that traffic.
pub fn egress_redirect_command(listen_port: impl Display) -> String {
    redirect_rules(listen_port).join(" && ")
}

/// The firewall a sandbox network namespace gets behind its egress proxy sidecar.
///
/// [`egress_redirect_command`] only steers TCP into the proxy; anything else (DNS and every other UDP flow, ICMP,
/// IPv6) would leave the namespace untouched. These rules close that:
///
/// - TCP from the workload is redirected to the proxy, as before.
/// - [`ProxyUid::EGRESS_PROXY`] may open TCP anywhere and send DNS queries: the proxy resolves names itself and
///   never trusts what the workload resolved.
/// - Loopback traffic to `127.0.0.1` / `::1` is accepted, which covers the redirected flows and the proxy's
///   explicit listener.
/// - Replies on connections opened from outside (published ports) are accepted.
/// - Everything else is rejected, including workload DNS, so the workload resolves names only by handing them to
///   the proxy, and a TCP flow the redirect missed (one opened before the rules existed) is cut.
pub fn egress_lockdown_rules(listen_port: impl Display) -> EgressLockdownRules {
    let proxy = ProxyUid::EGRESS_PROXY;
    let filter = |binary: &str, loopback: &str| {
        vec![
            format!("{binary} -A OUTPUT -p tcp -m owner --uid-owner {proxy} -j ACCEPT"),
            format!("{binary} -A OUTPUT -p udp --dport {DNS_PORT} -m owner --uid-owner {proxy} -j ACCEPT"),
            format!("{binary} -A OUTPUT -o lo -d {loopback} -j ACCEPT"),
            format!("{binary} -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED --ctdir REPLY -j ACCEPT"),
            format!("{binary} -A OUTPUT -j REJECT"),
        ]
    };
    let mut ipv4 = redirect_rules(listen_port);
    ipv4.extend(filter("iptables", "127.0.0.1"));
    EgressLockdownRules {
        ipv4,
        ipv6: filter("ip6tables", "::1"),
    }
}

/// Shell command installing [`egress_lockdown_rules`], stopping at the first rule that fails so the caller never
/// starts the proxy behind a partial firewall. The IPv6 rules are skipped only when the kernel has no IPv6 stack in
/// the namespace, since then there is no IPv6 egress to refuse.
pub fn egress_lockdown_command(listen_port: impl Display) -> String {
    let rules = egress_lockdown_rules(listen_port);
    format!(
        "{} && {{ [ ! -e {IPV6_STACK_PROBE} ] || {{ {}; }}; }}",
        rules.ipv4.join(" && "),
        rules.ipv6.join(" && ")
    )
}

fn redirect_rules(listen_port: impl Display) -> Vec<String> {
    vec![
        format!(
            "iptables -t nat -A OUTPUT -p tcp -m owner --uid-owner {} -j RETURN",
            ProxyUid::EGRESS_PROXY
        ),
        format!("iptables -t nat -A OUTPUT -p tcp ! -d 127.0.0.1 -j REDIRECT --to-port {listen_port}"),
    ]
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
