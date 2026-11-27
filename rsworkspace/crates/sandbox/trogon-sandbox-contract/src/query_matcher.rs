use serde::{Deserialize, Serialize};

use crate::StringMatcher;

/// A query string parameter a [`MatchRule`](crate::MatchRule) compares.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryMatcher {
    pub key: String,
    pub value: StringMatcher,
}
