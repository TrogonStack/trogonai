use std::fmt;

use a2a_redaction::SkillId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RewriteKind {
    Replaced,
    Removed,
    Masked,
}

impl RewriteKind {
    pub fn from_manifest_str(raw: &str) -> Option<Self> {
        match raw {
            "Replaced" | "replaced" => Some(Self::Replaced),
            "Removed" | "removed" => Some(Self::Removed),
            "Masked" | "masked" => Some(Self::Masked),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Replaced => "Replaced",
            Self::Removed => "Removed",
            Self::Masked => "Masked",
        }
    }

    pub fn infer_from_values(before: &serde_json::Value, after: &serde_json::Value) -> Self {
        if after.is_null() {
            Self::Removed
        } else {
            let _ = before;
            Self::Replaced
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactionRewrite {
    skill_id: SkillId,
    path_jsonpath: String,
    kind: RewriteKind,
}

impl RedactionRewrite {
    pub fn new(skill_id: SkillId, path_jsonpath: impl Into<String>, kind: RewriteKind) -> Self {
        Self {
            skill_id,
            path_jsonpath: path_jsonpath.into(),
            kind,
        }
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn path_jsonpath(&self) -> &str {
        &self.path_jsonpath
    }

    pub fn kind(&self) -> &RewriteKind {
        &self.kind
    }
}

impl fmt::Display for RedactionRewrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}@{}",
            self.skill_id.as_str(),
            self.kind.as_str(),
            self.path_jsonpath
        )
    }
}
