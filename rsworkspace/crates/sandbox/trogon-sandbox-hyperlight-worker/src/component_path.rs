use std::path::{Path, PathBuf};

use trogon_sandbox_contract::SandboxImage;

use crate::ComponentPathError;

/// Resolves the Wasm component an image names to a file under `root`.
///
/// The image is a logical name: a registry image or Hyperlight module reference contributes its
/// repository, and a name without an extension gets `.wasm`.
pub fn resolve_component_path(root: &Path, image: &SandboxImage) -> Result<PathBuf, ComponentPathError> {
    let name = match image {
        SandboxImage::Registry { reference, .. } if !reference.repository.is_empty() => &reference.repository,
        SandboxImage::HyperlightModule { reference } if !reference.repository.is_empty() => &reference.repository,
        _ => return Err(ComponentPathError::Unnamed),
    };
    if name.contains("..") || name.starts_with('/') {
        return Err(ComponentPathError::Escapes(name.clone()));
    }
    let mut candidate = root.join(name);
    if candidate.extension().is_none() {
        candidate.set_extension("wasm");
    }
    if !candidate.is_file() {
        return Err(ComponentPathError::NotFound(candidate));
    }
    Ok(candidate)
}
