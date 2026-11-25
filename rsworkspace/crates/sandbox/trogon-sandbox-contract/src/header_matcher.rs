use serde::{Deserialize, Serialize};

use crate::StringMatcher;

/// A request header a [`MatchRule`](crate::MatchRule) compares.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct HeaderMatcher {
    pub name: String,
    pub value: StringMatcher,
}
