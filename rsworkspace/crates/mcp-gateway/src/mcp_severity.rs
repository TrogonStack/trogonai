use std::fmt;

/// Severity of an MCP threat finding. Exactly three variants per
/// MCP-SECURITY-GATEWAY-1.0 section 6.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum McpSeverity {
    Info,
    Warning,
    Critical,
}

impl McpSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for McpSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_info_below_warning_below_critical() {
        assert!(McpSeverity::Info < McpSeverity::Warning);
        assert!(McpSeverity::Warning < McpSeverity::Critical);
    }

    #[test]
    fn has_exactly_three_variants_with_stable_names() {
        assert_eq!(McpSeverity::Info.as_str(), "info");
        assert_eq!(McpSeverity::Warning.as_str(), "warning");
        assert_eq!(McpSeverity::Critical.as_str(), "critical");
    }
}
