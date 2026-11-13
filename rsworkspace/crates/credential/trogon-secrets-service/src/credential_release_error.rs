/// What a refused release tells the requester.
///
/// Carries no reason on purpose: every denial looks the same from outside, so
/// a requester cannot probe which tenants, credentials, or policies exist. The
/// precise reason goes to the audit record and the logs only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialReleaseError {
    #[error("credential release denied")]
    Denied,
    #[error("requester xkey denied")]
    KeyDenied,
    #[error("credential release is unavailable")]
    Unavailable,
    #[error("credential release failed an integrity check")]
    IntegrityFailure,
    #[error("credential release failed internally")]
    Internal,
}

#[cfg(test)]
mod tests;
