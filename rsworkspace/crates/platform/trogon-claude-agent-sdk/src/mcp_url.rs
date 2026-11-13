#[derive(Clone)]
pub struct McpUrl(url::Url);

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
    pub fn new(value: &str) -> Result<Self, McpUrlError> {
        let value = url::Url::parse(value)?;
        if !matches!(value.scheme(), "http" | "https") {
            return Err(McpUrlError::Scheme);
        }
        if !value.username().is_empty() || value.password().is_some() {
            return Err(McpUrlError::Credentials);
        }
        if value.fragment().is_some() {
            return Err(McpUrlError::Fragment);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for McpUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpUrl { .. }")
    }
}
