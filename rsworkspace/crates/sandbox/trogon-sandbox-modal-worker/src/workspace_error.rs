use trogon_sandbox_contract::S3BucketNameError;
use trogon_sandbox_credential_client::ClientError;
use trogon_sandbox_volumes::{ManifestKeyError, VolumeError, VolumePrefixError};

use crate::CommitShaError;

/// A workspace could not be staged for, or collected from, a sandbox.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(
        "host_mount workspaces are not supported on the Modal provider: the sandbox runs in Modal's cloud and \
         cannot see this worker's filesystem"
    )]
    HostMountUnsupported,
    #[error("NATS KV secret refs are not resolved in the Modal worker yet")]
    SecretUnresolved,
    #[error("git {step} failed: {stderr}")]
    Git { step: &'static str, stderr: String },
    #[error("the clone url is invalid: {0}")]
    CloneUrl(#[from] url::ParseError),
    #[error("the clone url scheme {0} is not one git may fetch from")]
    CloneScheme(String),
    #[error("the clone url cannot carry credentials")]
    CloneCredentials,
    #[error("the commit is invalid: {0}")]
    CommitSha(#[from] CommitShaError),
    #[error("the volume bucket is invalid: {0}")]
    Bucket(#[from] S3BucketNameError),
    #[error("the volume prefix is invalid: {0}")]
    Prefix(#[from] VolumePrefixError),
    #[error("an object has no usable key: {0}")]
    Key(#[from] ManifestKeyError),
    #[error("the volume could not be read: {0}")]
    Volume(#[from] VolumeError),
    #[error("the credential broker refused: {0}")]
    Credential(#[from] ClientError),
    #[error("only S3 credential scopes can open a volume store")]
    ScopeUnsupported,
    #[error("expected volume credential token format access_key:secret_key[:session_token]")]
    CredentialToken,
    #[error("the object store failed: {0}")]
    ObjectStore(#[from] object_store::Error),
    #[error("the scratch directory failed: {0}")]
    Io(#[from] std::io::Error),
}
