#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CommandOwnerError {
    #[error("command custody has an invalid lease identity")]
    LeaseIdentity,
    #[error("command custody requires a canonical sandbox identity")]
    SandboxIdentity,
}
