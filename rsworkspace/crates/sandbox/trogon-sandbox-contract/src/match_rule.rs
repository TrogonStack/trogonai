use serde::{Deserialize, Serialize};

use crate::{HeaderMatcher, HttpMethod, QueryMatcher, StringMatcher};

/// Matcher DSL controlling which requests on a domain get transformed or forwarded versus passed through untouched.
///
/// `methods` uses OR semantics (any one match succeeds). `query_string` and `headers` use AND semantics across
/// entries (all must match), with any one repeated value satisfying an entry. Header names are compared
/// case-insensitively per RFC 9110, and query string keys and values are case-sensitive per RFC 3986.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MatchRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<StringMatcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<HttpMethod>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query_string: Vec<QueryMatcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderMatcher>,
}
