use trogon_sandbox_contract::{LeaseError, ProviderKind, SandboxImage};

use serde_json::Value;

use crate::check_runtime::check_runtime;
use crate::constants::{KNOWN_RUNTIMES, MAX_IMAGE_LEN};
use crate::json_object::object;

/// What a new sandbox starts from.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum Base {
    Runtime(String),
    Image(String),
    Snapshot(String),
}

impl Base {
    /// Known untagged legacy runtime names keep their runtime semantics. Other references identify managed or custom
    /// images; native snapshots restore their captured image.
    pub(crate) fn for_image(image: &SandboxImage, default_runtime: &str) -> Result<Self, LeaseError> {
        match image {
            SandboxImage::Registry { reference, .. } => {
                if reference.repository.is_empty() {
                    check_runtime(default_runtime)?;
                    return Ok(Self::Runtime(default_runtime.to_owned()));
                }
                if reference.registry.is_empty()
                    && reference.tag.is_none()
                    && reference.digest.is_none()
                    && KNOWN_RUNTIMES.contains(&reference.repository.as_str())
                {
                    return Ok(Self::Runtime(reference.repository.clone()));
                }
                let image = reference.allowlist_string();
                if image.len() > MAX_IMAGE_LEN {
                    return Err(LeaseError::InvalidSpec {
                        reason: format!("a Vercel image reference is at most {MAX_IMAGE_LEN} characters"),
                    });
                }
                Ok(Self::Image(image))
            }
            SandboxImage::NativeSnapshot { reference } => {
                if reference.provider != ProviderKind::Vercel {
                    return Err(LeaseError::InvalidSpec {
                        reason: "native snapshot belongs to a different provider".to_owned(),
                    });
                }
                Ok(Self::Snapshot(reference.id.as_str().to_owned()))
            }
            _ => Err(LeaseError::SnapshotUnsupported {
                provider: ProviderKind::Vercel,
                reason: "Vercel restores only its own native snapshots; name a Vercel runtime or image as a registry \
                         image"
                    .to_owned(),
            }),
        }
    }

    pub(crate) fn runtime(&self) -> Option<String> {
        match self {
            Self::Runtime(runtime) => Some(runtime.clone()),
            _ => None,
        }
    }

    pub(crate) fn image(&self) -> Option<String> {
        match self {
            Self::Image(image) => Some(image.clone()),
            _ => None,
        }
    }

    pub(crate) fn source(&self) -> Option<serde_json::Value> {
        match self {
            Self::Snapshot(id) => Some(object([
                ("type", Value::from("snapshot")),
                ("snapshotId", Value::from(id.as_str())),
            ])),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
