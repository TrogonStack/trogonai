use std::io;
use std::path::PathBuf;

use trogon_sandbox_oci::OciError;

/// Why a snapshot could not be packed, pushed, pulled or unpacked.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotTransportError {
    /// The layout directory could not be archived.
    #[error("cannot archive layout directory {}: {source}", path.display())]
    Pack {
        /// The directory being archived.
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The archive could not be compressed.
    #[error("cannot compress the snapshot archive: {0}")]
    Compress(#[source] io::Error),
    /// The blob is not a gzip stream.
    #[error("cannot decompress the snapshot archive: {0}")]
    Decompress(#[source] io::Error),
    /// The archive could not be written to disk.
    #[error("cannot unpack the snapshot archive into {}: {source}", path.display())]
    Unpack {
        /// The directory being filled.
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The artifact reference to push could not be built.
    #[error("cannot build the push target: {0}")]
    Target(#[source] OciError),
    /// The registry refused the push.
    #[error("cannot push the snapshot artifact: {0}")]
    Push(#[source] OciError),
    /// The registry refused the pull.
    #[error("cannot pull snapshot artifact {reference}: {source}")]
    Pull {
        /// The artifact that was requested.
        reference: String,
        #[source]
        source: OciError,
    },
    /// The artifact holds no snapshot layer.
    #[error("snapshot artifact has no layers")]
    NoLayers,
}
