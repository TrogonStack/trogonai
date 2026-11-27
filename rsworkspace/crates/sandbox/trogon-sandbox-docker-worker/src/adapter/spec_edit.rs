//! Rebuilds a [`SandboxSpec`] with one field replaced. The spec has no
//! field setters, so the edit is one parse through its only constructor.

use trogon_sandbox_contract::{SandboxImage, SandboxSpec, SandboxSpecError, WorkspaceSource};

pub(crate) fn with_workspace(spec: SandboxSpec, workspace: WorkspaceSource) -> Result<SandboxSpec, SandboxSpecError> {
    let mut parts = spec.into_parts();
    parts.workspace = workspace;
    SandboxSpec::new(parts)
}

pub(crate) fn with_image(spec: SandboxSpec, image: SandboxImage) -> Result<SandboxSpec, SandboxSpecError> {
    let mut parts = spec.into_parts();
    parts.image = image;
    SandboxSpec::new(parts)
}
