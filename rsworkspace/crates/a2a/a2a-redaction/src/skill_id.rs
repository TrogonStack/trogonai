use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

/// Skill identifier — a non-empty token usable as a path/filename segment.
///
/// Construction goes through `new` so an invalid `SkillId` is unrepresentable.
/// `Deserialize` re-uses the same validator so wire-borne values can't bypass
/// the gate via a transparent serde derive (which would otherwise admit
/// values containing path separators or `..`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SkillId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillIdError {
    #[error("skill id must be non-empty")]
    Empty,
    /// Forbid path separators and parent-dir traversal so the value is
    /// safe to interpolate into wasm-bundle filesystem paths.
    #[error("skill id contains forbidden character {0:?}")]
    ForbiddenCharacter(char),
    #[error("skill id contains path traversal sequence")]
    PathTraversal,
}

impl SkillId {
    pub fn new(id: impl Into<String>) -> Result<Self, SkillIdError> {
        let id = id.into();
        if id.is_empty() {
            return Err(SkillIdError::Empty);
        }
        if id == "." || id == ".." || id.contains("..") {
            return Err(SkillIdError::PathTraversal);
        }
        for ch in id.chars() {
            if matches!(ch, '/' | '\\' | '\0') || ch.is_control() {
                return Err(SkillIdError::ForbiddenCharacter(ch));
            }
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SkillId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        SkillId::new(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
