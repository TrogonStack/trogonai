use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{SecretBinding, SecretBindingsError};

/// The secret bindings of one sandbox.
///
/// It holds at most [`SecretBindings::MAX`] bindings, and no two share a name or a target. It serializes as a JSON
/// array in the order given, and construction and deserialization both enforce the rules. An empty collection is
/// valid.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<SecretBinding>", into = "Vec<SecretBinding>")]
pub struct SecretBindings(Vec<SecretBinding>);

impl SecretBindings {
    /// Most bindings one sandbox accepts.
    pub const MAX: usize = 64;

    pub fn new(bindings: Vec<SecretBinding>) -> Result<Self, SecretBindingsError> {
        if bindings.len() > Self::MAX {
            return Err(SecretBindingsError::TooMany {
                max: Self::MAX,
                got: bindings.len(),
            });
        }
        let mut names = HashSet::new();
        let mut targets = HashSet::new();
        for binding in &bindings {
            if !names.insert(binding.name()) {
                return Err(SecretBindingsError::DuplicateName(binding.name().clone()));
            }
            if !targets.insert(binding.target()) {
                return Err(SecretBindingsError::DuplicateTarget(binding.target().clone()));
            }
        }
        Ok(Self(bindings))
    }

    pub fn bindings(&self) -> &[SecretBinding] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<Vec<SecretBinding>> for SecretBindings {
    type Error = SecretBindingsError;

    fn try_from(bindings: Vec<SecretBinding>) -> Result<Self, Self::Error> {
        Self::new(bindings)
    }
}

impl From<SecretBindings> for Vec<SecretBinding> {
    fn from(bindings: SecretBindings) -> Self {
        bindings.0
    }
}
