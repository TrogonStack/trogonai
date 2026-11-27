use trogon_sandbox_contract::LeaseId;

/// The lease named by an SSH user of the form `<lease-id>@gw`.
///
/// The lease id becomes a NATS subject token, so one with whitespace or subject syntax is refused.
pub(crate) fn parse_lease_user(user: &str) -> Option<LeaseId> {
    let lease = user.split('@').next()?.trim();
    let subject_safe = lease
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'.' | b'*' | b'>'));
    (!lease.is_empty() && subject_safe).then(|| LeaseId::new(lease))
}
