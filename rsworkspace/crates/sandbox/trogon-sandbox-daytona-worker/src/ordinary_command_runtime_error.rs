use crate::DaytonaApiError;

/// Why an ordinary command runtime could not be loaded, or could not be installed in a sandbox.
#[derive(Debug, thiserror::Error)]
pub enum OrdinaryCommandRuntimeError {
    #[error("ordinary command artifact requires a lowercase SHA256 pin")]
    PinNotLowercaseHex,
    #[error("open ordinary command artifact: {0}")]
    Open(#[source] std::io::Error),
    #[error("read ordinary command artifact: {0}")]
    Read(#[source] std::io::Error),
    #[error("ordinary command artifact must be a bounded regular file")]
    NotBoundedRegularFile,
    #[error("ordinary command artifact does not match its configured SHA256 pin")]
    PinMismatch,
    #[error("bundled ordinary command bridge is invalid")]
    BridgeInvalid,
    #[error("ordinary command runtime requires a Linux ELF64 artifact")]
    NotLinuxElf64,
    #[error("ordinary command runtime ELF ABI is unsupported")]
    ElfAbiUnsupported,
    #[error("ordinary command runtime architecture is unsupported")]
    ArchitectureUnsupported,
    #[error("ordinary command runtime ELF headers are invalid")]
    ElfHeadersInvalid,
    #[error("ordinary command artifact is truncated")]
    ArtifactTruncated,
    #[error("ordinary command runtime must not use a guest interpreter")]
    GuestInterpreter,
    #[error("ordinary runtime guest operation is unavailable: {0}")]
    GuestUnavailable(#[source] DaytonaApiError),
    #[error("ordinary runtime guest operation did not produce bounded success")]
    GuestNotBounded,
    #[error("guest runtime identity is malformed")]
    GuestIdentityMalformed,
    #[error("ordinary commands require Linux and Node 22.7 or newer")]
    GuestUnsupported,
    #[error("guest command runtime architecture is unsupported")]
    GuestArchitectureUnsupported,
    #[error("configured ordinary command artifact does not match the exact guest architecture")]
    GuestArchitectureMismatch,
    #[error("ordinary runtime directory creation is unconfirmed")]
    DirectoryUnconfirmed,
    #[error("ordinary command {what} upload failed: {source}")]
    Upload {
        what: &'static str,
        #[source]
        source: DaytonaApiError,
    },
    #[error("ordinary command {what} permissions failed: {source}")]
    Permissions {
        what: &'static str,
        #[source]
        source: DaytonaApiError,
    },
    #[error("ordinary runtime probe was not exact")]
    ProbeNotExact,
    #[error("ordinary runtime cleanup is unconfirmed")]
    CleanupUnconfirmed,
}
