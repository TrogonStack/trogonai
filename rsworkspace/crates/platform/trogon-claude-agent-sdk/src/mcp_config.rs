use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// Python `mcp_servers` given as a `str` or `Path`: a config file path or JSON text sent verbatim as its own
/// `--mcp-config` value. JSON text can carry credentials, so debug output never shows it.
#[derive(Clone, Eq, PartialEq)]
pub struct McpConfig(OsString);

impl McpConfig {
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

impl std::fmt::Debug for McpConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl From<OsString> for McpConfig {
    fn from(value: OsString) -> Self {
        Self(value)
    }
}

impl From<PathBuf> for McpConfig {
    fn from(value: PathBuf) -> Self {
        Self(value.into_os_string())
    }
}

impl From<String> for McpConfig {
    fn from(value: String) -> Self {
        Self(value.into())
    }
}

impl From<&str> for McpConfig {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
