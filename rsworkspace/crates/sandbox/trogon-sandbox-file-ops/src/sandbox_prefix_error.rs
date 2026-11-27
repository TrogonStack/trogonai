/// A string cannot name where the served tree appears inside the sandbox.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SandboxPrefixError {
    #[error("sandbox prefix must be an absolute path")]
    NotAbsolute,
    #[error("sandbox prefix must not contain a NUL byte")]
    ContainsNul,
    #[error("sandbox prefix must not contain empty, `.`, or `..` segments")]
    NotNormalized,
}
