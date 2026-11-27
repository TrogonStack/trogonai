use std::path::PathBuf;

use trogon_sandbox_fly_worker::{FlyConfigError, FlyRegionError};
use trogon_sandbox_modal_worker::{ModalApiError, ModalAppNameError};
use trogon_sandbox_proxmox_worker::{ProxmoxConfigError, VmIdError};
use trogon_sandbox_space_worker::MaxLeasesError;
use trogon_sandbox_volumes::VolumeRegionError;

use crate::constants::REGENERATE_COMMAND;

/// Why the matrix could not be rendered, written, or checked.
#[derive(Debug, thiserror::Error)]
pub enum MatrixError {
    #[error("serializing the profile of {label}: {source}")]
    Serialize {
        label: &'static str,
        source: serde_json::Error,
    },
    #[error("the document has no {0}")]
    MissingMarker(&'static str),
    #[error("the end marker comes before the begin marker")]
    MarkersOutOfOrder,
    #[error("reading {path}: {source}", path = .path.display())]
    Read { path: PathBuf, source: std::io::Error },
    #[error("writing {path}: {source}", path = .path.display())]
    Write { path: PathBuf, source: std::io::Error },
    #[error("{path} is out of date with the worker capability profiles; regenerate with: {REGENERATE_COMMAND}", path = .0.display())]
    Stale(PathBuf),
    #[error("the modal app name: {0}")]
    ModalAppName(#[from] ModalAppNameError),
    #[error("the modal volume region: {0}")]
    ModalVolumeRegion(#[from] VolumeRegionError),
    #[error("the modal bridge: {0}")]
    ModalBridge(#[from] ModalApiError),
    #[error("the fly region: {0}")]
    FlyRegion(#[from] FlyRegionError),
    #[error("the fly config: {0}")]
    FlyConfig(#[from] FlyConfigError),
    #[error("the proxmox template: {0}")]
    ProxmoxTemplate(#[from] VmIdError),
    #[error("the proxmox config: {0}")]
    ProxmoxConfig(#[from] ProxmoxConfigError),
    #[error("the space lease capacity: {0}")]
    SpaceMaxLeases(#[from] MaxLeasesError),
    #[error("unknown argument {0}; pass --write to regenerate or nothing to check")]
    UnknownArgument(String),
}
