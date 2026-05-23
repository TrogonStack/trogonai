use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillId(String);

impl SkillId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
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

impl From<&str> for SkillId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip() {
        let id = SkillId::new("skill-a");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"skill-a\"");
        let parsed: SkillId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn display_matches_inner() {
        let id = SkillId::from("x");
        assert_eq!(format!("{id}"), "x");
    }

    #[test]
    fn from_str_construction() {
        assert_eq!(SkillId::from("y").as_str(), "y");
    }
}
