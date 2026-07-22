/// Domain failure reported by a guest decider across the WIT boundary.
///
/// This mirrors the WIT `domain-error` record verbatim: a stable machine
/// `code`, a human-readable `message`, and the `details` pairs the guest
/// attached (the bridge populates these with the causal chain below the
/// top-level message). It carries no source error because the guest error
/// already crossed a serialization boundary; the `details` pairs are all
/// that survives of the original chain.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct DomainErrorDetail {
    /// Stable, machine-readable rejection code the guest reported.
    pub code: String,
    /// Human-readable description of the failure.
    pub message: String,
    /// Ordered key/value pairs carrying the error's source chain, most specific cause last.
    pub details: Vec<(String, String)>,
}

impl From<trogon_decider_wit::host::DomainError> for DomainErrorDetail {
    fn from(value: trogon_decider_wit::host::DomainError) -> Self {
        Self {
            code: value.code,
            message: value.message,
            details: value.details,
        }
    }
}

#[cfg(test)]
mod tests;
