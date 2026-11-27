/// A string is not a sandbox user name [`SandboxUser`](crate::SandboxUser) accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SandboxUserError {
    #[error("sandbox user must not be empty")]
    Empty,
}
