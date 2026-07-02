use std::fmt;

/// How the gateway handles threats detected in a tool response.
///
/// Exactly three variants per MCP-SECURITY-GATEWAY-1.0 MUST-4, mirroring
/// AGT's `ResponsePolicy` enum (`agent_os/mcp_gateway.py`):
/// - [`Self::Block`]: deny the response entirely when any threat is found.
/// - [`Self::Sanitize`]: strip injection tags but still block
///   credential/PII/exfiltration leaks (those cannot be safely stripped
///   from prose).
/// - [`Self::Log`]: allow the response through but record every detected
///   threat.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ResponsePolicy {
    #[default]
    Block,
    Sanitize,
    Log,
}

impl ResponsePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Sanitize => "sanitize",
            Self::Log => "log",
        }
    }
}

impl fmt::Display for ResponsePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_exactly_three_variants_with_stable_names() {
        assert_eq!(ResponsePolicy::Block.as_str(), "block");
        assert_eq!(ResponsePolicy::Sanitize.as_str(), "sanitize");
        assert_eq!(ResponsePolicy::Log.as_str(), "log");
    }

    #[test]
    fn defaults_to_block() {
        assert_eq!(ResponsePolicy::default(), ResponsePolicy::Block);
    }
}
