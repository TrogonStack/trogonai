use crate::SuiteFailureError;

/// The checks a worker failed, so it is not conformant.
#[derive(Debug, thiserror::Error)]
#[error("worker is not conformant:\n  {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n  "))]
pub struct NotConformantError(pub Vec<SuiteFailureError>);

#[cfg(test)]
mod tests;
