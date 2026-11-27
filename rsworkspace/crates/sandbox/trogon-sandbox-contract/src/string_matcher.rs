use serde::{Deserialize, Serialize};

/// A single matcher value comparison.
///
/// `Regex` stores the raw RE2 pattern string; compiling it is left to the component that enforces the policy.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StringMatcher {
    Exact(String),
    StartsWith(String),
    Regex(String),
}
