/// An external MCP server endpoint, stored and sent verbatim like both upstream SDKs do.
///
/// `McpUrl::from` keeps any text. `McpUrl::validated` is an opt-in check that neither upstream SDK
/// performs: it refuses non-HTTP(S) schemes, embedded credentials and fragments, and still keeps the
/// original text unchanged.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct McpUrl(String);

#[derive(Debug, thiserror::Error)]
pub enum McpUrlError {
    #[error("invalid MCP endpoint URL")]
    Parse(#[from] url::ParseError),
    #[error("MCP HTTP endpoints require HTTP or HTTPS")]
    Scheme,
    #[error("MCP endpoint credentials must be passed separately")]
    Credentials,
    #[error("MCP endpoints cannot contain fragments")]
    Fragment,
}

impl McpUrl {
    pub fn validated(value: &str) -> Result<Self, McpUrlError> {
        let parsed = url::Url::parse(value)?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(McpUrlError::Scheme);
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(McpUrlError::Credentials);
        }
        if parsed.fragment().is_some() {
            return Err(McpUrlError::Fragment);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for McpUrl {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for McpUrl {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Debug for McpUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpUrl { .. }")
    }
}
