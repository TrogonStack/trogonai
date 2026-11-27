use serde::{Deserialize, Serialize};

/// One result of a file search. A glob-only search yields matches with no line.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
