use trogon_sandbox_contract::SandboxImage;

use crate::v8_wasm_lease_error::V8WasmLeaseError;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum GuestKind {
    Js,
    Wasm,
}

impl GuestKind {
    pub(crate) fn parse(value: &str) -> Result<Self, V8WasmLeaseError> {
        match value {
            "js" => Ok(Self::Js),
            "wasm" => Ok(Self::Wasm),
            other => Err(V8WasmLeaseError::UnsupportedGuestKind(other.to_owned())),
        }
    }

    /// The guest kind an image names. A registry reference also covers placeholder images, so an
    /// unrecognized repository falls through to the environment variable instead of failing.
    pub(crate) fn from_image(image: &SandboxImage) -> Option<Self> {
        let candidate = match image {
            SandboxImage::HyperlightModule { reference } => reference.repository.as_str(),
            SandboxImage::Registry { reference, .. } => reference.repository.as_str(),
            SandboxImage::Snapshot { .. } | SandboxImage::SnapshotTag { .. } | SandboxImage::NativeSnapshot { .. } => {
                return None;
            }
        };
        match candidate {
            "js" => Some(Self::Js),
            "wasm" => Some(Self::Wasm),
            other if other.ends_with(".js") => Some(Self::Js),
            other if other.ends_with(".wasm") => Some(Self::Wasm),
            _ => None,
        }
    }
}
