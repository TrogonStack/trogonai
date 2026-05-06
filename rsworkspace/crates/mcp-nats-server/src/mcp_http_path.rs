#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpHttpPath(String);

impl McpHttpPath {
    pub fn new(path: impl Into<String>) -> Result<Self, McpHttpPathError> {
        let path = path.into();
        if !is_valid_http_path(&path) {
            return Err(McpHttpPathError);
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpHttpPathError;

impl std::fmt::Display for McpHttpPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MCP HTTP path must start with '/', include a route segment, and contain only valid path characters"
        )
    }
}

impl std::error::Error for McpHttpPathError {}

fn is_valid_http_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && path
            .chars()
            .all(|c| c.is_ascii() && !c.is_control() && !c.is_whitespace() && !matches!(c, '?' | '#' | '\\'))
        && path.split('/').all(|segment| !matches!(segment, "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_http_paths() {
        for path in ["/mcp", "/tenant/mcp", "/mcp-v1", "/mcp_v1", "/mcp~v1"] {
            assert_eq!(McpHttpPath::new(path).unwrap().as_str(), path);
        }
    }

    #[test]
    fn rejects_missing_root_empty_unsafe_or_relative_segments() {
        for path in [
            "",
            "/",
            "mcp",
            "/ ",
            "/path with space",
            "/path\n",
            "/path?query",
            "/path#fragment",
            "/path\\segment",
            "/.",
            "/..",
            "/tenant/../mcp",
            "/unicode/é",
        ] {
            assert!(McpHttpPath::new(path).is_err(), "{path:?} should be invalid");
        }
    }

    #[test]
    fn error_display_is_specific() {
        assert_eq!(
            McpHttpPathError.to_string(),
            "MCP HTTP path must start with '/', include a route segment, and contain only valid path characters"
        );
    }
}
