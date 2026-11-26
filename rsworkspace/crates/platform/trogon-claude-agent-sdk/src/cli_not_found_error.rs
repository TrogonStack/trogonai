/// No Claude Code executable was found on `PATH` or in the well-known install locations.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct CliNotFoundError {
    pub(crate) message: &'static str,
}

impl CliNotFoundError {
    pub fn message(&self) -> &'static str {
        self.message
    }
}
