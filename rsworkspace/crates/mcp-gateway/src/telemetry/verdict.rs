use crate::{ServerName, ToolName, Verdict};
use tracing::{info, warn};

/// Record a discovery-scan or tool-call interception decision. Blocks are
/// logged at `warn` (an operator-actionable signal); allows and flags are
/// logged at `info`.
pub fn record_verdict(server_name: &ServerName, tool_name: &ToolName, verdict: &Verdict) {
    match verdict {
        Verdict::Allow => {
            info!(
                mcp.gateway.server_name = %server_name,
                mcp.gateway.tool_name = %tool_name,
                mcp.gateway.verdict = "allow",
                "gateway allowed MCP traffic",
            );
        }
        Verdict::Flag { threats } => {
            info!(
                mcp.gateway.server_name = %server_name,
                mcp.gateway.tool_name = %tool_name,
                mcp.gateway.verdict = "flag",
                mcp.gateway.threat_count = threats.len(),
                "gateway flagged MCP traffic",
            );
        }
        Verdict::Block { reason, threats } => {
            warn!(
                mcp.gateway.server_name = %server_name,
                mcp.gateway.tool_name = %tool_name,
                mcp.gateway.verdict = "block",
                mcp.gateway.threat_count = threats.len(),
                mcp.gateway.reason = %reason,
                "gateway blocked MCP traffic",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test only: `tracing` macros without a subscriber installed are
    /// no-ops, so this just proves the call sites type-check for every
    /// `Verdict` variant.
    #[test]
    fn records_every_verdict_variant_without_panicking() {
        let server = ServerName::new("web-tools").expect("valid");
        let tool = ToolName::new("search").expect("valid");

        record_verdict(&server, &tool, &Verdict::Allow);
        record_verdict(&server, &tool, &Verdict::Flag { threats: Vec::new() });
        record_verdict(
            &server,
            &tool,
            &Verdict::Block {
                reason: "denied".to_string(),
                threats: Vec::new(),
            },
        );
    }
}
