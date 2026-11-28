use std::path::PathBuf;

use crate::{EgressRulesFailure, ProxyUidWorkloadError, VolumeCacheError};

/// Failures of the Docker engine adapter and the local Docker provider.
#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error("failed to connect to Docker socket")]
    Connect(#[source] bollard::errors::Error),
    #[error("{operation} failed")]
    Api {
        operation: &'static str,
        #[source]
        source: bollard::errors::Error,
    },
    #[error("container state missing")]
    ContainerStateMissing,
    #[error("wait_container returned no result")]
    WaitReturnedNothing,
    #[error("create checkpoint dir {dir}")]
    CheckpointDir {
        dir: String,
        #[source]
        source: std::io::Error,
    },
    #[error("checkpoint {checkpoint_id} not visible after create")]
    CheckpointNotVisible { checkpoint_id: String },
    #[error("encode checkpoint start query")]
    CheckpointQuery(#[source] serde_urlencoded::ser::Error),
    #[error("build checkpoint start request")]
    CheckpointRequestBuild(#[source] http::Error),
    #[error("checkpoint start request failed")]
    CheckpointRequest(#[source] hyper_util::client::legacy::Error),
    #[error("checkpoint start failed ({status}): {body}")]
    CheckpointRejected { status: http::StatusCode, body: String },
    #[error("sandbox spec cannot set both clone and host_mount")]
    CloneAndHostMount,
    #[error("privileged workloads require K8sMicroVm or KubeVirt; LocalDocker only provides Container-tier isolation")]
    PrivilegedUnsupported,
    #[error("NatsKv SecretRef is not supported in LocalDockerProvider; use Literal for local testing")]
    NatsKvSecretUnsupported,
    #[error("clone_url must contain ://")]
    CloneUrlMissingScheme,
    #[error("git {command} failed")]
    GitSpawn {
        command: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("git {command} failed: {stderr}")]
    GitFailed { command: &'static str, stderr: String },
    #[error("{what} path is not valid utf-8: {path:?}")]
    NonUtf8Path { what: &'static str, path: PathBuf },
    #[error("create egress ca file {path:?}")]
    EgressCaFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("egress sidecar readiness marker {path:?}")]
    EgressReadyMarker {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The sidecar never confirmed its firewall, so the sandbox was torn down rather than left with open egress.
    #[error("egress sidecar {sidecar_id} did not install its firewall")]
    EgressRulesNotInstalled {
        sidecar_id: String,
        #[source]
        cause: EgressRulesFailure,
    },
    #[error("serialize network policy to json")]
    PolicySerialize(#[source] serde_json::Error),
    #[error("write egress policy file {path:?}")]
    PolicyWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("policy file path has no file name: {path:?}")]
    PolicyPathHasNoName { path: PathBuf },
    #[error("rename egress policy temp file {from:?} onto {to:?}")]
    PolicyRename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A multi-step operation failed; `source` is the underlying cause.
    #[error("{operation}")]
    Stage {
        operation: &'static str,
        #[source]
        source: Box<DockerError>,
    },
    #[error(transparent)]
    ProxyUidWorkload(#[from] ProxyUidWorkloadError),
    #[error(transparent)]
    VolumeCache(#[from] VolumeCacheError),
}

impl DockerError {
    /// Wraps a bollard failure with the Engine API call that produced it.
    pub(crate) fn api(operation: &'static str) -> impl FnOnce(bollard::errors::Error) -> Self {
        move |source| Self::Api { operation, source }
    }

    /// Wraps this error as the cause of a failed higher-level operation.
    pub(crate) fn during(self, operation: &'static str) -> Self {
        Self::Stage {
            operation,
            source: Box::new(self),
        }
    }
}
