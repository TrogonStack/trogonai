/// Domain failure reported by a guest decider across the WIT boundary.
///
/// This mirrors the WIT `domain-error` record verbatim: a stable machine
/// `code` and a human-readable `message`. It carries no source error because
/// the guest error already crossed a serialization boundary; there is nothing
/// further to preserve on the host side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainErrorDetail {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for DomainErrorDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DomainErrorDetail {}

impl From<trogon_decider_wit::host::DomainError> for DomainErrorDetail {
    fn from(value: trogon_decider_wit::host::DomainError) -> Self {
        Self {
            code: value.code,
            message: value.message,
        }
    }
}

#[cfg(test)]
mod tests;
