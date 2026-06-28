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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RedactionRewriteError {
    #[error("redaction rewrite path must not be empty")]
    EmptyPath,
}

impl RedactionRewrite {
    /// Construct an audit-bound redaction rewrite. Rejects empty paths
    /// so a malformed entry can't land in the audit subject and silently
    /// stand in for a real rewrite.
    pub fn new(
        skill_id: SkillId,
        path_jsonpath: impl Into<String>,
        kind: RewriteKind,
    ) -> Result<Self, RedactionRewriteError> {
        let path_jsonpath = path_jsonpath.into();
        if path_jsonpath.trim().is_empty() {
            return Err(RedactionRewriteError::EmptyPath);
        }
        Ok(Self {
            skill_id,
            path_jsonpath,
            kind,
        })
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
