use std::path::PathBuf;

/// Why `spec.image` does not name a component under the component root.
#[derive(Debug, thiserror::Error)]
pub enum ComponentPathError {
    /// The image names no component.
    #[error("spec.image must name a Wasm component file under component_root")]
    Unnamed,
    /// The image is absolute or climbs out of the component root.
    #[error("spec.image must be a relative path without `..` segments (got {0:?})")]
    Escapes(String),
    /// No file exists at the resolved path.
    #[error("wasm component {} not found under component_root", .0.display())]
    NotFound(PathBuf),
}
