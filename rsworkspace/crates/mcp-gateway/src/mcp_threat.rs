use crate::{McpSeverity, McpThreatType, ServerName, ToolName};

/// A single threat finding from an MCP tool scan.
///
/// AGT's `MCPThreat.details` is an untyped `dict[str, Any]`; here it is
/// replaced by `McpThreatDetails`, a closed set of the detail shapes the
/// scanners actually produce, so callers can match on a real Rust type
/// instead of probing a dynamic bag of values.
#[derive(Clone, Debug, PartialEq)]
pub struct McpThreat {
    threat_type: McpThreatType,
    severity: McpSeverity,
    tool_name: ToolName,
    server_name: ServerName,
    message: String,
    matched_pattern: Option<String>,
    details: McpThreatDetails,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum McpThreatDetails {
    #[default]
    None,
    CharOrd(u32),
    CommentPreview(String),
    FieldName(String),
    RugPull {
        changed_description: bool,
        changed_schema: bool,
        version: u32,
    },
    Impersonation {
        original_server: ServerName,
    },
    Typosquat {
        similar_tool: ToolName,
        similar_server: ServerName,
    },
}

impl McpThreat {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        threat_type: McpThreatType,
        severity: McpSeverity,
        tool_name: ToolName,
        server_name: ServerName,
        message: impl Into<String>,
        matched_pattern: Option<String>,
        details: McpThreatDetails,
    ) -> Self {
        Self {
            threat_type,
            severity,
            tool_name,
            server_name,
            message: message.into(),
            matched_pattern,
            details,
        }
    }

    pub fn threat_type(&self) -> McpThreatType {
        self.threat_type
    }

    pub fn severity(&self) -> McpSeverity {
        self.severity
    }

    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub fn server_name(&self) -> &ServerName {
        &self.server_name
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn matched_pattern(&self) -> Option<&str> {
        self.matched_pattern.as_deref()
    }

    pub fn details(&self) -> &McpThreatDetails {
        &self.details
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_all_constructed_fields() {
        let threat = McpThreat::new(
            McpThreatType::HiddenInstruction,
            McpSeverity::Critical,
            ToolName::new("search").expect("valid"),
            ServerName::new("web-tools").expect("valid"),
            "Invisible unicode characters detected",
            Some(r"[\u{200b}]".to_string()),
            McpThreatDetails::CharOrd(0x200b),
        );
        assert_eq!(threat.threat_type(), McpThreatType::HiddenInstruction);
        assert_eq!(threat.severity(), McpSeverity::Critical);
        assert_eq!(threat.tool_name().as_str(), "search");
        assert_eq!(threat.server_name().as_str(), "web-tools");
        assert_eq!(threat.message(), "Invisible unicode characters detected");
        assert_eq!(threat.matched_pattern(), Some(r"[\u{200b}]"));
        assert_eq!(threat.details(), &McpThreatDetails::CharOrd(0x200b));
    }
}
