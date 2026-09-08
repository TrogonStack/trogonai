//! How a deployment names the module it wants, per ADR#0058.
//!
//! The reference and the object key are one type with two renderings rather
//! than two strings a caller assembles. They differ only because the
//! object-name grammar excludes `@`, and a projection that lives at its own
//! call sites is a projection that can be written one way here and another way
//! there.

use std::str::FromStr;

use trogon_decider_wasm_runtime::{ModuleName, ModuleNameError, ModuleVersion, ModuleVersionError};

use crate::constants::{MODULE_OBJECT_KEY_SEPARATOR, MODULE_REFERENCE_SEPARATOR};

/// One module, named the way its own descriptor names it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleReference {
    name: ModuleName,
    version: ModuleVersion,
}

impl ModuleReference {
    /// Pairs a name with a version.
    pub const fn new(name: ModuleName, version: ModuleVersion) -> Self {
        Self { name, version }
    }

    /// Returns the module name half.
    pub const fn name(&self) -> &ModuleName {
        &self.name
    }

    /// Returns the module version half.
    pub const fn version(&self) -> &ModuleVersion {
        &self.version
    }

    /// The key this module is stored under in an object store.
    pub fn object_key(&self) -> String {
        format!("{}{MODULE_OBJECT_KEY_SEPARATOR}{}", self.name, self.version)
    }

    /// The filename this module is stored under on a filesystem.
    pub fn file_name(&self) -> String {
        format!("{self}.wasm")
    }
}

impl std::fmt::Display for ModuleReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{MODULE_REFERENCE_SEPARATOR}{}", self.name, self.version)
    }
}

impl FromStr for ModuleReference {
    type Err = ModuleReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (name, version) =
            value
                .split_once(MODULE_REFERENCE_SEPARATOR)
                .ok_or_else(|| ModuleReferenceError::MissingVersion {
                    value: value.to_owned(),
                })?;
        Ok(Self::new(
            ModuleName::new(name).map_err(|source| ModuleReferenceError::Name {
                value: name.to_owned(),
                source,
            })?,
            ModuleVersion::new(version).map_err(|source| ModuleReferenceError::Version {
                value: version.to_owned(),
                source,
            })?,
        ))
    }
}

/// Why a written reference does not name a module.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModuleReferenceError {
    /// The reference named no version, so it names a family rather than a build.
    #[error("module reference '{value}' has no '{MODULE_REFERENCE_SEPARATOR}version' suffix")]
    MissingVersion { value: String },
    /// The name half is not a module name.
    #[error("module reference name '{value}' is invalid: {source}")]
    Name {
        value: String,
        #[source]
        source: ModuleNameError,
    },
    /// The version half is not a module version.
    #[error("module reference version '{value}' is invalid: {source}")]
    Version {
        value: String,
        #[source]
        source: ModuleVersionError,
    },
}

#[cfg(test)]
mod tests;
