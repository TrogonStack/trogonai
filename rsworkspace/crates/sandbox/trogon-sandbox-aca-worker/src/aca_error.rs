use trogon_sandbox_contract::{LeaseError, ProviderKind, SandboxIdError};
use trogon_sandbox_credential_client::ClientError;
use trogon_sandbox_protocol::AuthorityResourceIdError;
use trogon_sandbox_volumes::{StoreResourceNameError, VolumeError, VolumePrefixError};

/// Why an Azure Container Apps operation failed.
#[derive(Debug, thiserror::Error)]
pub enum AcaError {
    /// The lease spec asks for something this provider cannot do.
    #[error("invalid lease spec: {0}")]
    InvalidSpec(String),
    /// The lease asks to boot from a snapshot, which Dynamic Sessions cannot restore.
    #[error("snapshot unsupported on ACA: {0}")]
    SnapshotUnsupported(String),
    /// The Dynamic Sessions data plane answered with an error status.
    #[error("ACA API error ({status}): {message}")]
    AcaApi { status: u16, message: String },
    /// The data plane answered a session lookup with a body that does not state a status.
    #[error("unreadable ACA session description: {0}")]
    SessionStatus(#[from] crate::SessionStatusError),
    /// A cancel subject names no lease.
    #[error("cancel subject names no lease: {0:?}")]
    CancelSubject(String),
    /// The session never became ready.
    #[error("ACA session not ready after {attempts} attempts")]
    SessionNotReady { attempts: u32 },
    /// Azure authentication failed.
    #[error("Azure authentication failed: {0}")]
    Auth(String),
    /// The credential broker refused or could not be reached.
    #[error("credential broker: {0}")]
    Credential(#[from] ClientError),
    /// The volume credential is not `account:key[:session_token]`.
    #[error("credential broker: expected volume credential token format account:key[:session_token]")]
    VolumeCredentialFormat,
    /// The workspace could not be materialized.
    #[error("workspace materialization failed: {0}")]
    Workspace(String),
    /// A workspace file or process failed.
    #[error("workspace materialization failed: {0}")]
    Io(#[from] std::io::Error),
    /// A volume could not be pulled or pushed.
    #[error("volume sync failed: {0}")]
    Volume(#[from] VolumeError),
    /// The volume prefix is not a valid object-store prefix.
    #[error("volume sync failed: {0}")]
    Prefix(#[from] VolumePrefixError),
    /// The volume bucket or storage account is not a valid resource name.
    #[error("volume sync failed: {0}")]
    StoreName(#[from] StoreResourceNameError),
    /// A request could not be sent or its response could not be read.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    /// A data plane URL could not be built.
    #[error("invalid ACA URL: {0}")]
    Url(#[from] url::ParseError),
    /// The session cannot be tracked as an authority resource.
    #[error(transparent)]
    Resource(#[from] AuthorityResourceIdError),
    /// The lease or session id is not a valid sandbox id.
    #[error(transparent)]
    SandboxId(#[from] SandboxIdError),
    /// The allocation failed and so did the cleanup that followed.
    #[error("{primary}; allocation cleanup remains pending: {cleanup}")]
    AllocationCleanup {
        primary: Box<AcaError>,
        cleanup: Box<AcaError>,
    },
}

impl AcaError {
    /// An error status the data plane answered with.
    pub fn aca_api(status: u16, message: impl Into<String>) -> Self {
        Self::AcaApi {
            status,
            message: message.into(),
        }
    }

    /// The lease error a caller outside this worker sees.
    pub fn into_lease_error(self) -> LeaseError {
        match self {
            Self::InvalidSpec(reason) => LeaseError::InvalidSpec { reason },
            Self::SnapshotUnsupported(reason) => LeaseError::SnapshotUnsupported {
                provider: ProviderKind::AzureContainerApps,
                reason,
            },
            other => LeaseError::Backend {
                message: other.to_string(),
            },
        }
    }
}
