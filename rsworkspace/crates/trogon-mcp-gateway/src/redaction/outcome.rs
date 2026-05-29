use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteEntry {
    pub path: String,
    pub op: String,
}

impl RewriteEntry {
    #[must_use]
    pub fn new(path: impl Into<String>, op: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            op: op.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionOutcome {
    pub rewrites: Vec<RewriteEntry>,
}

impl RedactionOutcome {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            rewrites: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionApplyResult {
    /// No rules configured for this tool/direction.
    Passthrough,
    /// Rules ran and may have rewritten fields.
    Applied(RedactionOutcome),
    /// Rules exist but schema was unavailable; payload left unchanged.
    Skipped { reason: String },
}
