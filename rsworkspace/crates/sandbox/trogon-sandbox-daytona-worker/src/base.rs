use trogon_sandbox_contract::{LeaseError, ProviderKind, SandboxImage};

use crate::constants::MAX_SNAPSHOT_NAME_LEN;

/// The name of the Daytona snapshot a new sandbox starts from.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Base(String);

impl Base {
    /// A registry image names a Daytona snapshot by its reference; an empty repository takes the default snapshot. A
    /// native snapshot of this provider restores by its own name.
    pub(crate) fn for_image(image: &SandboxImage, default_snapshot: &str) -> Result<Self, LeaseError> {
        let name = match image {
            SandboxImage::Registry { reference, .. } if reference.repository.is_empty() => default_snapshot.to_owned(),
            SandboxImage::Registry { reference, .. } => reference.allowlist_string(),
            SandboxImage::NativeSnapshot { reference } => {
                if reference.provider != ProviderKind::Daytona {
                    return Err(LeaseError::InvalidSpec {
                        reason: "native snapshot belongs to a different provider".to_owned(),
                    });
                }
                reference.id.as_str().to_owned()
            }
            _ => {
                return Err(LeaseError::SnapshotUnsupported {
                    provider: ProviderKind::Daytona,
                    reason: "Daytona starts from its own snapshots; name a Daytona snapshot as a registry image"
                        .to_owned(),
                });
            }
        };
        if name.is_empty() || name.len() > MAX_SNAPSHOT_NAME_LEN {
            return Err(LeaseError::InvalidSpec {
                reason: format!("a Daytona snapshot name is 1 to {MAX_SNAPSHOT_NAME_LEN} characters"),
            });
        }
        Ok(Self(name))
    }

    pub(crate) fn into_name(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests;
