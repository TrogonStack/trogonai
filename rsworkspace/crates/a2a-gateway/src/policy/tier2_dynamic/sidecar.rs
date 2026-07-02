use std::path::{Path, PathBuf};
use std::{fs, io};

use super::dynamic_condition::{
    DynamicConditionToml, Tier2DynamicCondition, Tier2DynamicConditionError, convert_dynamic_condition,
};

/// Failure surface for loading a `<rule>.dynamic.toml` sidecar. Mirrors
/// `crate::policy::tier2_cel::compiler::CelCompileError`'s two-layer
/// shape: I/O and TOML syntax at this level, condition schema validation
/// nested via `Schema`.
#[derive(Debug, thiserror::Error)]
pub enum Tier2DynamicSidecarError {
    #[error("read dynamic condition sidecar {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("parse dynamic condition sidecar {}", path.display())]
    ParseToml {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("dynamic condition sidecar {} is invalid: {error}", path.display())]
    Schema {
        path: PathBuf,
        error: Tier2DynamicConditionError,
    },
}

/// Load the optional `<rule>.dynamic.toml` sidecar next to a `.cel` rule
/// file. Returns `Ok(None)` when no sidecar exists -- a `.cel` rule with
/// no sidecar has no dynamic condition and evaluates on CEL alone.
///
/// Any sidecar that DOES exist but fails to parse or fails schema
/// validation is a hard load error (fail-closed), matching how a
/// malformed `.cel` file fails the whole bundle load rather than being
/// silently skipped.
pub fn load_dynamic_condition_sidecar(
    cel_path: &Path,
) -> Result<Option<Tier2DynamicCondition>, Tier2DynamicSidecarError> {
    let sidecar_path = sidecar_path_for(cel_path);
    if !sidecar_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&sidecar_path).map_err(|source| Tier2DynamicSidecarError::Read {
        path: sidecar_path.clone(),
        source,
    })?;
    let parsed: DynamicConditionToml = toml::from_str(&raw).map_err(|source| Tier2DynamicSidecarError::ParseToml {
        path: sidecar_path.clone(),
        source: Box::new(source),
    })?;
    let condition = convert_dynamic_condition(parsed).map_err(|error| Tier2DynamicSidecarError::Schema {
        path: sidecar_path.clone(),
        error,
    })?;
    Ok(Some(condition))
}

fn sidecar_path_for(cel_path: &Path) -> PathBuf {
    let mut sidecar = cel_path.to_path_buf();
    sidecar.set_extension("dynamic.toml");
    sidecar
}
