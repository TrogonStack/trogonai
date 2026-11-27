use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{Digest, OciImageRef};

use crate::ResolvedContainerImageError;
use crate::resolved_container_image_wire::ResolvedContainerImageWire;

/// A container image pinned to a digest, with the runtime settings read from its config.
///
/// The reference carries the digest and no tag, the environment names are non-empty, and the
/// working directory is absolute. Construction and deserialization both enforce this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ResolvedContainerImageWire", into = "ResolvedContainerImageWire")]
pub struct ResolvedContainerImage {
    reference: OciImageRef,
    digest: Digest,
    env: BTreeMap<String, String>,
    working_dir: Option<String>,
    user: Option<String>,
    compressed_bytes: u64,
}

impl ResolvedContainerImage {
    pub fn new(
        reference: OciImageRef,
        digest: Digest,
        env: BTreeMap<String, String>,
        working_dir: Option<String>,
        user: Option<String>,
        compressed_bytes: u64,
    ) -> Result<Self, ResolvedContainerImageError> {
        if reference.digest.as_ref() != Some(&digest) || reference.tag.is_some() {
            return Err(ResolvedContainerImageError::NotPinned);
        }
        for (name, value) in &env {
            if name.is_empty() || name.contains('\0') {
                return Err(ResolvedContainerImageError::InvalidEnvName);
            }
            if value.contains('\0') {
                return Err(ResolvedContainerImageError::InvalidEnvValue);
            }
        }
        if working_dir
            .as_ref()
            .is_some_and(|dir| !Path::new(dir).is_absolute() || dir.contains('\0'))
        {
            return Err(ResolvedContainerImageError::InvalidWorkingDir);
        }
        Ok(Self {
            reference,
            digest,
            env,
            working_dir,
            user,
            compressed_bytes,
        })
    }

    /// The image reference, pinned to [`Self::digest`].
    pub fn reference(&self) -> &OciImageRef {
        &self.reference
    }

    /// Digest of the selected image manifest.
    pub fn digest(&self) -> &Digest {
        &self.digest
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    pub fn working_dir(&self) -> Option<&str> {
        self.working_dir.as_deref()
    }

    /// The image config's `USER`, when it sets one.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Sum of the config and layer sizes the manifest declares.
    pub fn compressed_bytes(&self) -> u64 {
        self.compressed_bytes
    }
}

impl TryFrom<ResolvedContainerImageWire> for ResolvedContainerImage {
    type Error = ResolvedContainerImageError;

    fn try_from(wire: ResolvedContainerImageWire) -> Result<Self, Self::Error> {
        Self::new(
            wire.reference,
            wire.digest,
            wire.env,
            wire.working_dir,
            wire.user,
            wire.compressed_bytes,
        )
    }
}

impl From<ResolvedContainerImage> for ResolvedContainerImageWire {
    fn from(image: ResolvedContainerImage) -> Self {
        Self {
            reference: image.reference,
            digest: image.digest,
            env: image.env,
            working_dir: image.working_dir,
            user: image.user,
            compressed_bytes: image.compressed_bytes,
        }
    }
}
