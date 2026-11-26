use trogon_sandbox_contract::{DriveError, FileOpErrorKind, LeaseError, TerminalError};

use crate::error_class::ErrorClass;

/// Caller-facing classification of a wire error.
///
/// Every implementation is an exhaustive `match` with no wildcard arm, so adding a variant to the
/// classified type fails to compile until it is classified.
pub trait Classified {
    /// How a caller should react to this error.
    fn class(&self) -> ErrorClass;
}

impl Classified for LeaseError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::QuotaExceeded { .. } => {
                ErrorClass::permanent("the tenant's resource quota is used up; release leases or raise the quota")
            }
            Self::NoProviderAvailable => {
                ErrorClass::transient("no worker is registered for the requested providers; retry once one is online")
            }
            Self::InvalidSpec { .. } => ErrorClass::permanent("the sandbox spec is invalid; fix the request"),
            Self::InvalidNetworkPolicy { .. } => {
                ErrorClass::permanent("the network policy is invalid; fix the request")
            }
            Self::Backend { .. } => {
                ErrorClass::transient("a broker backend call failed; retry with the same idempotency key")
            }
            Self::Unauthorized { .. } => ErrorClass::permanent("the credentials are not allowed to do this"),
            Self::NoWorkerSatisfiesCapabilities { .. } => ErrorClass::permanent(
                "no worker declares the required capabilities; relax the requirements or pick another provider",
            ),
            Self::NoWorkerCanRestoreSnapshot { .. } => {
                ErrorClass::permanent("no worker can restore this snapshot; choose a provider that supports its format")
            }
            Self::TooManyActiveLeases { .. } => {
                ErrorClass::permanent("the tenant is at its active lease cap; release a lease before creating another")
            }
            Self::TooManyTaggedLeases { .. } => ErrorClass::permanent(
                "this user (tag value) is at its active lease cap; release one of its leases first",
            ),
            Self::SnapshotQuotaExceeded { .. } => {
                ErrorClass::permanent("the tenant's snapshot quota is used up; delete a snapshot")
            }
            Self::SnapshotTagNotFound { .. } => {
                ErrorClass::permanent("the snapshot tag does not exist; check its name")
            }
            Self::SnapshotUnsupported { .. } => ErrorClass::permanent(
                "the provider does not support snapshots for this lease; use a provider that does",
            ),
            Self::SnapshotSignatureInvalid { .. } => {
                ErrorClass::permanent("the snapshot signature failed verification; the artifact is untrusted")
            }
            Self::PrewarmQuotaExceeded { .. } => ErrorClass::permanent("the tenant's prewarm quota is used up"),
            Self::ImageNotReady { .. } => {
                ErrorClass::transient("the image is still being prepared; retry after a short wait")
            }
            Self::SourceLeaseNotFound { .. } => {
                ErrorClass::permanent("the source lease does not exist or already ended")
            }
            Self::SourceLeaseNotRunning { .. } => {
                ErrorClass::permanent("the source lease is not running; resume or recreate it")
            }
            Self::CheckpointRunningUnsupported { .. } => {
                ErrorClass::permanent("the provider cannot checkpoint a running lease; stop it first")
            }
            Self::CheckpointRunningFailed { .. } => {
                ErrorClass::transient("checkpointing the running lease failed; retry")
            }
            Self::SnapshotsDisabled { .. } => {
                ErrorClass::permanent("the lease was created with snapshots off; no disk capture of it is allowed")
            }
            Self::LeaseNotFound { .. } => ErrorClass::permanent("the lease does not exist or already ended"),
            Self::LeaseNotRunning { .. } => ErrorClass::permanent("the lease is not running; resume or recreate it"),
            Self::InvalidTags { .. } => ErrorClass::permanent("the tags are invalid; fix keys and values"),
            Self::WouldExceedMaxLifetime { .. } => {
                ErrorClass::permanent("the extension would pass the maximum lease lifetime; ask for less")
            }
            Self::ProviderLaunchFailed { .. } => {
                ErrorClass::transient("the provider failed to start the sandbox; retry, possibly on another provider")
            }
            Self::ProviderRejectedDispatch { .. } => {
                ErrorClass::transient("the provider rejected the dispatch; retry, possibly on another provider")
            }
            Self::FileTooLargeForTransport { .. } => {
                ErrorClass::permanent("the file exceeds the transport payload limit; use chunked transfer")
            }
            Self::CommandOutputTooLargeForTransport { .. } => ErrorClass::permanent(
                "the command output exceeds the transport payload limit; stream it or write it to a file",
            ),
            Self::FilesTooLargeForTransport { .. } => {
                ErrorClass::permanent("the files exceed the transport payload limit; split the batch")
            }
            Self::NameConflict { .. } => ErrorClass::transient(
                "another request is creating a sandbox under this name; retry with the same idempotency key",
            ),
            Self::NativePassthroughNotPermitted { .. } => ErrorClass::permanent(
                "the tenant policy does not allow vendor passthrough; drop `native` or ask an admin to enable it",
            ),
            Self::InvalidNativeOptions { .. } => ErrorClass::permanent(
                "the worker rejected the native options; use only the keys its documentation lists",
            ),
            Self::NoWorkerInRegion { .. } => ErrorClass::transient(
                "no worker serves the requested regions right now; retry later or add fallback regions",
            ),
            Self::OperationConflict { .. } => ErrorClass::permanent(
                "the operation id was used for a different request, or the lease has another operation in flight; pick a new id or wait",
            ),
            Self::OperationCanceled { .. } => ErrorClass::permanent("the operation was canceled"),
            Self::OperationNotFound { .. } => ErrorClass::permanent("no operation with this id exists for the tenant"),
            Self::SuspendUnsupported { .. } => ErrorClass::permanent(
                "the provider cannot suspend to that level; ask for auto or pick a supported level",
            ),
            Self::LeaseWaking { .. } => ErrorClass::transient("the lease is waking; retry after the hinted delay"),
            Self::SuspendArtifactLost { .. } => {
                ErrorClass::permanent("the suspend artifact is gone, so the lease cannot wake; release it and recreate")
            }
            Self::EntityUnavailable { .. } => {
                ErrorClass::transient("the sandbox entity store failed; retry with the same idempotency key")
            }
            Self::EntityNotEnabled => ErrorClass::permanent(
                "the broker has sandbox entities off for this operation; ask an operator to set SANDBOX_ENTITY_MODE",
            ),
        }
    }
}

impl Classified for DriveError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::DriveNotFound { .. } => ErrorClass::permanent("the drive does not exist"),
            Self::DriveAlreadyExists { .. } => ErrorClass::permanent("a drive with this name already exists"),
            Self::DriveAttached { .. } => {
                ErrorClass::permanent("the drive is attached to another lease; release that lease first")
            }
            Self::DriveSizeExceeded { .. } => ErrorClass::permanent("the requested drive size is over the limit"),
            Self::TooManyDrives { .. } => ErrorClass::permanent("too many drives for one lease; mount fewer"),
            Self::DriveMountPathOverlap { .. } => ErrorClass::permanent("two drive mounts overlap; use distinct paths"),
            Self::InvalidDriveMountPath { .. } => ErrorClass::permanent("the drive mount path is invalid"),
            Self::StorageBackend { .. } => ErrorClass::transient("the drive storage backend failed; retry"),
            Self::Unauthorized { .. } => ErrorClass::permanent("the credentials are not allowed to do this"),
        }
    }
}

impl Classified for TerminalError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::NotFound => {
                ErrorClass::permanent("no open terminal has this id; it was closed, exited or never opened")
            }
            Self::IdInUse => ErrorClass::permanent("the terminal id is open on another lease; pick a new id"),
            Self::TooManyTerminals { .. } => {
                ErrorClass::permanent("the lease has too many open terminals; close one first")
            }
            Self::InvalidRequest { .. } => ErrorClass::permanent("the terminal request is invalid; fix the request"),
            Self::SpawnFailed { .. } => ErrorClass::transient("the terminal process could not start; retry"),
            Self::Exited => ErrorClass::permanent("the terminal's process has exited; close it and open a new one"),
        }
    }
}

impl Classified for FileOpErrorKind {
    fn class(&self) -> ErrorClass {
        match *self {
            Self::InvalidPath => ErrorClass::permanent("the path is malformed or escapes the sandbox filesystem"),
            Self::PermissionDenied => ErrorClass::permanent("the sandbox user may not access this path"),
            Self::AlreadyExists => ErrorClass::permanent("the path already exists"),
            Self::NotADirectory => ErrorClass::permanent("a path component is not a directory"),
            Self::IsADirectory => ErrorClass::permanent("the path is a directory"),
            Self::DirectoryNotEmpty => ErrorClass::permanent("the directory is not empty"),
            Self::TooLarge => ErrorClass::permanent("the request or result exceeds a size limit"),
            Self::InvalidRequest => ErrorClass::permanent("the request is malformed"),
            Self::Io => ErrorClass::transient("the filesystem reported an I/O error; retry"),
            Self::UnknownCheckpoint => {
                ErrorClass::permanent("the checkpoint is not held on this lease; create a new one")
            }
            Self::UnknownWatch => ErrorClass::permanent("the watch is not live on this lease; start a new one"),
        }
    }
}
