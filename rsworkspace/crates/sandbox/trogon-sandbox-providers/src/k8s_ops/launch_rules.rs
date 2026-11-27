use trogon_sandbox_contract::NetworkPolicy;

use crate::RunAsUserError;

/// The `LABEL_DIGEST` value for `digest`. A label value is at most 63
/// characters with no `:`, so `sha256:<64 hex>` is stored as its hex,
/// shortened to fit; that keeps far more than enough bits to tell digests apart.
pub fn digest_label_value(digest: &str) -> String {
    let hex = digest.split_once(':').map_or(digest, |(_, hex)| hex);
    hex.chars().filter(char::is_ascii_alphanumeric).take(63).collect()
}

/// Whether enforcing `policy` takes the egress proxy sidecar, which a pod that is
/// already running cannot be given. Domain rules, denied domains and request
/// rules are all decided by that proxy, so a `NetworkPolicy` object alone would
/// enforce something looser than the lease asked for.
pub fn policy_needs_egress_proxy(policy: &NetworkPolicy) -> bool {
    match policy {
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
        NetworkPolicy::UserDefined {
            rules,
            denied_domains,
            request_rules,
            ..
        } => !rules.is_empty() || !denied_domains.is_empty() || !request_rules.is_empty(),
    }
}

/// Parses a lease user (`"uid"` or `"uid:gid"`) into k8s
/// `run_as_user`/`run_as_group` values. Kubernetes `SecurityContext` only
/// accepts numeric ids, unlike Docker/subprocess, so a username-form value is
/// rejected rather than silently dropped. Root (uid `0`) is rejected outright:
/// sandboxes must run as a non-root uid.
pub(crate) fn parse_run_as_user(user: &str) -> Result<(i64, Option<i64>), RunAsUserError> {
    let not_numeric = || RunAsUserError::NotNumeric { user: user.to_string() };
    let (uid_str, gid_str) = match user.split_once(':') {
        Some((uid, gid)) => (uid, Some(gid)),
        None => (user, None),
    };

    let uid: i64 = uid_str.parse().map_err(|_| not_numeric())?;
    let gid: Option<i64> = gid_str.map(|g| g.parse().map_err(|_| not_numeric())).transpose()?;

    if uid == 0 || gid == Some(0) {
        return Err(RunAsUserError::Root { user: user.to_string() });
    }

    Ok((uid, gid))
}

/// `runtimeClassName` for a configured runtime class. An empty name leaves the
/// field unset so the pod runs on the cluster's default runtime, for clusters
/// that register no kata/gvisor `RuntimeClass`.
pub(crate) fn runtime_class_name(runtime_class: &str) -> Option<String> {
    (!runtime_class.is_empty()).then(|| runtime_class.to_string())
}

/// Whether a k8s `RuntimeClass` name looks like a strong-isolation class
/// (Kata/Firecracker microVM or gVisor user-space kernel) as opposed to plain
/// `runc`. The runtime classes are operator-configurable, so the provider
/// cannot assume its resolved runtime class is always microVM-tier and must
/// check before granting `privileged` elevated capabilities.
pub(crate) fn is_strong_isolation_runtime_class(runtime_class: &str) -> bool {
    let lower = runtime_class.to_ascii_lowercase();
    lower.contains("kata") || lower.contains("gvisor")
}
