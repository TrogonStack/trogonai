/// A string is not a Mach service name [`MachServiceName`](crate::MachServiceName) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum MachServiceNameError {
    #[error("a Mach service name cannot be empty")]
    Empty,
    #[error("`{0}` has a wildcard that is not a single trailing `*`")]
    InvalidWildcard(String),
    #[error("a Mach service name cannot hold the control character {0:?}")]
    ControlCharacter(char),
}
