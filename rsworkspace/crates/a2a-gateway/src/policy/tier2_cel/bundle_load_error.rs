use std::path::Path;

use crate::policy::tier2_dynamic::Tier2DynamicSidecarError;

use super::compiler::CelCompileError;

/// Failure surface for loading (or refreshing) a `Tier2CompiledBundle`.
/// Wraps the two independent things that can go wrong for a rule entry:
/// the `.cel` program itself, or its optional `.dynamic.toml` sidecar.
/// A malformed sidecar fails the whole bundle load, matching the
/// existing fail-closed treatment of a malformed `.cel` file -- an
/// operator typo shouldn't silently drop a dynamic condition and leave
/// the CEL-only half of the rule active.
#[derive(Debug, thiserror::Error)]
pub enum Tier2BundleLoadError {
    #[error(transparent)]
    Cel(#[from] CelCompileError),
    #[error(transparent)]
    DynamicSidecar(#[from] Tier2DynamicSidecarError),
}

impl Tier2BundleLoadError {
    /// The offending file path, regardless of whether the failure came
    /// from the `.cel` program or its `.dynamic.toml` sidecar.
    pub fn path(&self) -> &Path {
        match self {
            Self::Cel(err) => err.path(),
            Self::DynamicSidecar(
                Tier2DynamicSidecarError::Read { path, .. }
                | Tier2DynamicSidecarError::ParseToml { path, .. }
                | Tier2DynamicSidecarError::Schema { path, .. },
            ) => path,
        }
    }
}
