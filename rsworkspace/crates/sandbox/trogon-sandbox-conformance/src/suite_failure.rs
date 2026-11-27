use crate::CheckFailureError;

/// A check that did not pass, by name.
#[derive(Debug, thiserror::Error)]
#[error("{name}: {failure}")]
pub struct SuiteFailureError {
    pub name: &'static str,
    #[source]
    pub failure: CheckFailureError,
}

#[cfg(test)]
mod tests;
