use trogon_sandbox_contract::{
    DeniedDomain, DeniedDomainReasons, DenyReason, DenyReasonError, Host, NetworkOptions, NetworkPolicy,
};

use crate::{StandardDenyReason, denied_domain_matches};

/// The reason recorded for `entry`, or the standard deny-list reason when the entry has none.
///
/// # Errors
///
/// Returns the error of [`DenyReason::new`], which never fires for the fixed standard text.
pub fn denied_domain_reason(
    reasons: &DeniedDomainReasons,
    entry: &DeniedDomain,
) -> Result<DenyReason, DenyReasonError> {
    match reasons.get(entry) {
        Some(reason) => Ok(reason.clone()),
        None => StandardDenyReason::OnDenyList.reason(),
    }
}

/// The reason of the first entry of `denied` that matches the destination, or `None` when no entry does.
///
/// # Errors
///
/// Returns the error of [`DenyReason::new`], which never fires for the fixed standard text.
pub fn explain_denied_destination(
    reasons: &DeniedDomainReasons,
    denied: &[DeniedDomain],
    host: &Host,
    port: u16,
) -> Result<Option<DenyReason>, DenyReasonError> {
    denied
        .iter()
        .find(|entry| denied_domain_matches(entry, host, port))
        .map(|entry| denied_domain_reason(reasons, entry))
        .transpose()
}

/// The words for refusing `host:port` because a denied domain matches it, or `None` when the policy lists no matching
/// denied domain.
///
/// # Errors
///
/// Returns the error of [`DenyReason::new`], which never fires for the fixed standard text.
pub fn deny_list_reason(
    options: &NetworkOptions,
    policy: &NetworkPolicy,
    host: &Host,
    port: u16,
) -> Result<Option<DenyReason>, DenyReasonError> {
    let NetworkPolicy::UserDefined { denied_domains, .. } = policy else {
        return Ok(None);
    };
    explain_denied_destination(&options.denied_domain_reasons, denied_domains, host, port)
}
