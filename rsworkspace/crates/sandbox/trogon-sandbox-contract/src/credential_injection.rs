use serde::{Deserialize, Serialize};

/// Where a broker-issued credential is injected into a matched request.
///
/// It is an enum rather than an optional header name plus an optional query key, so that "inject into both a
/// header and a query parameter" is unrepresentable, matching the [`DomainRuleAction`](crate::DomainRuleAction)
/// convention of using enums to rule out nonsensical combinations.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialInjection {
    /// Sets (overwriting any existing value) the named request header to the issued token value, for example
    /// `Header("Authorization".to_string())`.
    Header(String),
    /// Sets (overwriting any existing value) the named query-string parameter to the issued token value.
    Query(String),
}
