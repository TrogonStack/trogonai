//! Bridge-owned HTTPS / JWT bearer value objects (`AGENTS.md` — avoid primitive obsession).

use std::fmt;

/// Raw HTTPS `Authorization` header value (typically `Bearer …`, excluding header name).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallerHttpsAuth(String);

impl CallerHttpsAuth {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for CallerHttpsAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// Account-bound NATS User JWT minted via auth callout for this HTTPS caller identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeUserJwt(String);

impl BridgeUserJwt {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for BridgeUserJwt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("<redacted>")
    }
}
