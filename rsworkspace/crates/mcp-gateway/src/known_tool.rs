use crate::{ServerName, ToolName};

/// A previously-observed `(tool_name, server_name)` pair, supplied by the
/// caller as the comparison set for typosquat and cross-server-impersonation
/// detection. This mirrors the entries AGT's `MCPSecurityScanner` keeps in
/// its `_tool_by_name` index, but the gateway (not this library) owns
/// registry storage and lifetime; the scanner functions here are pure.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KnownTool {
    tool_name: ToolName,
    server_name: ServerName,
}

impl KnownTool {
    pub fn new(tool_name: ToolName, server_name: ServerName) -> Self {
        Self { tool_name, server_name }
    }

    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub fn server_name(&self) -> &ServerName {
        &self.server_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_tool_and_server_name() {
        let known = KnownTool::new(
            ToolName::new("search").expect("valid"),
            ServerName::new("web-tools").expect("valid"),
        );
        assert_eq!(known.tool_name().as_str(), "search");
        assert_eq!(known.server_name().as_str(), "web-tools");
    }
}
